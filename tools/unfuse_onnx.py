# Rewrites onnxruntime-optimized models back to standard ONNX so they run
# under tract: com.microsoft::FusedConv -> Conv + Relu/Clip.
#
# Usage: python unfuse_onnx.py model.onnx [out.onnx]
# (pip install onnx)

import sys

import onnx
from onnx import helper, numpy_helper
import numpy as np

CONV_ATTRS = {"dilations", "group", "kernel_shape", "pads", "strides"}


def unfuse(model):
    graph = model.graph
    new_nodes = []
    extra_inits = []
    changed = 0

    for node in graph.node:
        if not (node.domain == "com.microsoft" and node.op_type == "FusedConv"):
            new_nodes.append(node)
            continue
        changed += 1

        attrs = {a.name: a for a in node.attribute}
        activation = attrs["activation"].s.decode() if "activation" in attrs else ""
        params = list(attrs["activation_params"].floats) if "activation_params" in attrs else []

        conv_out = node.output[0] + "_unfused_conv" if activation else node.output[0]
        conv = helper.make_node(
            "Conv",
            inputs=list(node.input),
            outputs=[conv_out],
            name=node.name + "_conv" if node.name else "",
        )
        conv.attribute.extend(a for a in node.attribute if a.name in CONV_ATTRS)
        new_nodes.append(conv)

        if activation == "Relu":
            new_nodes.append(
                helper.make_node("Relu", [conv_out], [node.output[0]])
            )
        elif activation == "Clip":
            lo, hi = (params + [0.0, 6.0])[:2] if len(params) >= 2 else (0.0, 6.0)
            lo_name = node.output[0] + "_clip_min"
            hi_name = node.output[0] + "_clip_max"
            extra_inits.append(numpy_helper.from_array(np.float32(lo), lo_name))
            extra_inits.append(numpy_helper.from_array(np.float32(hi), hi_name))
            new_nodes.append(
                helper.make_node("Clip", [conv_out, lo_name, hi_name], [node.output[0]])
            )
        elif activation:
            raise SystemExit(f"unhandled FusedConv activation: {activation}")

    del graph.node[:]
    graph.node.extend(new_nodes)
    graph.initializer.extend(extra_inits)

    # Drop the Microsoft opset imports now that no custom ops remain.
    keep = [o for o in model.opset_import if not o.domain.startswith("com.microsoft")]
    del model.opset_import[:]
    model.opset_import.extend(keep)
    return changed


def main():
    src = sys.argv[1]
    dst = sys.argv[2] if len(sys.argv) > 2 else src
    model = onnx.load(src)
    n = unfuse(model)
    onnx.checker.check_model(model)
    onnx.save(model, dst)
    print(f"unfused {n} FusedConv nodes -> {dst}")


if __name__ == "__main__":
    main()
