# Prepares the OpenSeeFace/AITrack landmark model for the tract inference
# engine (pure-Rust, used by the CorsaConnect server):
#
#   1. com.microsoft::FusedConv  ->  Conv + Relu/Clip
#   2. Resize(linear, align_corners, sizes) -> two MatMuls with constant
#      interpolation matrices (mathematically identical for this model's
#      exact-2x upsamples)
#   3. onnx-simplifier constant folding with a static 1x3x224x224 input
#
# Usage: python tractify_lm.py lm_f.onnx [out.onnx]
# (pip install onnx onnxsim numpy)

import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper

from unfuse_onnx import unfuse


def interp_matrix(dst, src):
    """Rows of align_corners linear interpolation weights, shape [dst, src]."""
    m = np.zeros((dst, src), dtype=np.float32)
    for i in range(dst):
        pos = i * (src - 1) / (dst - 1) if dst > 1 else 0.0
        lo = int(np.floor(pos))
        w = pos - lo
        if lo + 1 < src:
            m[i, lo] = 1.0 - w
            m[i, lo + 1] = w
        else:
            m[i, lo] = 1.0
    return m


def resize_to_matmul(model):
    graph = model.graph
    inits = {i.name: i for i in graph.initializer}
    # Infer input spatial sizes: value_info may be missing, so read the sizes
    # constant for the destination and derive the source from the exact-2x rule.
    new_nodes = []
    changed = 0
    for node in graph.node:
        if node.op_type != "Resize":
            new_nodes.append(node)
            continue

        attrs = {a.name: a for a in node.attribute}
        mode = attrs["mode"].s.decode() if "mode" in attrs else "nearest"
        coord = (
            attrs["coordinate_transformation_mode"].s.decode()
            if "coordinate_transformation_mode" in attrs
            else "half_pixel"
        )
        sizes_name = node.input[3] if len(node.input) > 3 else None
        if mode != "linear" or coord != "align_corners" or sizes_name not in inits:
            raise SystemExit(f"unhandled Resize variant: {mode}/{coord}")
        sizes = numpy_helper.to_array(inits[sizes_name])
        dst_h, dst_w = int(sizes[2]), int(sizes[3])
        # This model only ever doubles resolution.
        src_h, src_w = (dst_h + 1) // 2, (dst_w + 1) // 2
        changed += 1

        x = node.input[0]
        out = node.output[0]
        a_h = numpy_helper.from_array(interp_matrix(dst_h, src_h), out + "_interp_h")
        a_wt = numpy_helper.from_array(
            interp_matrix(dst_w, src_w).T.copy(), out + "_interp_wT"
        )
        graph.initializer.extend([a_h, a_wt])
        mid = out + "_interp_mid"
        # [dh,sh] @ [1,C,sh,sw] -> [1,C,dh,sw], then @ [sw,dw] -> [1,C,dh,dw]
        new_nodes.append(helper.make_node("MatMul", [out + "_interp_h", x], [mid]))
        new_nodes.append(helper.make_node("MatMul", [mid, out + "_interp_wT"], [out]))

    del graph.node[:]
    graph.node.extend(new_nodes)
    return changed


def main():
    src = sys.argv[1]
    dst = sys.argv[2] if len(sys.argv) > 2 else src
    import onnxsim

    model = onnx.load(src)
    n_fused = unfuse(model)
    # First simplify folds the Shape/Slice chains so Resize sizes become
    # constants; then we can rewrite the resizes and fold once more.
    model, ok = onnxsim.simplify(
        model, overwrite_input_shapes={"input": [1, 3, 224, 224]}
    )
    if not ok:
        raise SystemExit("onnxsim could not validate the unfused model")
    n_resize = resize_to_matmul(model)
    onnx.checker.check_model(model)
    model, ok = onnxsim.simplify(model)
    if not ok:
        raise SystemExit("onnxsim could not validate the final model")

    onnx.save(model, dst)
    print(f"unfused {n_fused} convs, replaced {n_resize} resizes -> {dst}")


if __name__ == "__main__":
    main()
