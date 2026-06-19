package com.corsaconnect

import android.util.Log
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress

/**
 * Owns the two UDP links to the PC server:
 *   * a sender thread streaming [Protocol.Input] at ~60Hz, and
 *   * a receiver thread listening for telemetry on [Protocol.TELEMETRY_PORT].
 *
 * Plain threads (no coroutines) keep the dependency surface small and the
 * 60Hz loop predictable.
 */
class NetworkService(
    private val serverIp: String,
    private val inputProvider: () -> Protocol.Input,
    private val onTelemetry: (Protocol.Telemetry) -> Unit,
) {
    @Volatile private var running = false
    private var senderThread: Thread? = null
    private var receiverThread: Thread? = null
    private var sendSocket: DatagramSocket? = null
    private var recvSocket: DatagramSocket? = null

    fun start() {
        if (running) return
        running = true
        senderThread = Thread(::senderLoop, "cc-sender").apply { start() }
        receiverThread = Thread(::receiverLoop, "cc-receiver").apply { start() }
    }

    fun stop() {
        running = false
        sendSocket?.close()
        recvSocket?.close()
        senderThread = null
        receiverThread = null
    }

    private fun senderLoop() {
        try {
            val socket = DatagramSocket().also { sendSocket = it }
            val address = InetAddress.getByName(serverIp)
            while (running) {
                val bytes = Protocol.encodeInput(inputProvider())
                socket.send(DatagramPacket(bytes, bytes.size, address, Protocol.INPUT_PORT))
                Thread.sleep(16) // ~60Hz
            }
        } catch (e: Exception) {
            if (running) Log.w("CorsaConnect", "sender stopped: ${e.message}")
        }
    }

    private fun receiverLoop() {
        try {
            val socket = DatagramSocket(Protocol.TELEMETRY_PORT).also { recvSocket = it }
            val buf = ByteArray(64)
            while (running) {
                val packet = DatagramPacket(buf, buf.size)
                socket.receive(packet)
                Protocol.decodeTelemetry(packet.data, packet.length)?.let(onTelemetry)
            }
        } catch (e: Exception) {
            if (running) Log.w("CorsaConnect", "receiver stopped: ${e.message}")
        }
    }
}
