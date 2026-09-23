package com.corsaconnect

import android.content.Context
import android.net.wifi.WifiManager
import android.util.Log
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketAddress

/**
 * Owns the UDP link to the PC server: one socket on [Protocol.TELEMETRY_PORT]
 * that both sends [Protocol.Input] at ~100Hz and receives telemetry and the
 * PC's beacon.
 *
 * One socket, not two, is what lets the PC run without a firewall rule. The PC
 * speaks first - it broadcasts a beacon from its :5000 to our :5001 - and
 * Windows Firewall lets in replies to traffic the PC sent. Our input only
 * counts as a reply if it comes back from the port the PC is talking to, so it
 * has to leave from :5001 too.
 *
 * The socket is open for as long as the activity lives, connected or not, so
 * the start screen can show (and fill in) the PC it heard from.
 *
 * Plain threads (no coroutines) keep the dependency surface small and the
 * send loop predictable.
 */
class NetworkService(
    context: Context,
    private val inputProvider: () -> Protocol.Input,
    private val onTelemetry: (Protocol.Telemetry) -> Unit,
    private val onServerFound: (String) -> Unit,
) {
    // Some phones drop broadcast packets on Wi-Fi to save power unless an app
    // holds this lock; without it the beacon may never arrive.
    private val multicastLock: WifiManager.MulticastLock? =
        (context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
            ?.createMulticastLock("corsaconnect-beacon")
            ?.apply { setReferenceCounted(false) }

    @Volatile private var running = false
    @Volatile private var socket: DatagramSocket? = null
    /** The thread streaming input, or null while disconnected. */
    @Volatile private var senderThread: Thread? = null
    private var receiverThread: Thread? = null

    /** Bind the socket and start listening. Call once, from the activity. */
    fun open() {
        if (running) return
        running = true
        try { multicastLock?.acquire() } catch (e: Exception) {
            Log.w("CorsaConnect", "no multicast lock: ${e.message}")
        }
        val s = DatagramSocket(null as SocketAddress?)
        socket = try {
            s.reuseAddress = true
            s.broadcast = true
            s.bind(InetSocketAddress(Protocol.TELEMETRY_PORT))
            s
        } catch (e: Exception) {
            // Something else holds :5001. Input still gets through (but may need
            // a firewall rule on the PC); telemetry and discovery won't.
            Log.w("CorsaConnect", "could not bind :${Protocol.TELEMETRY_PORT}: ${e.message}")
            s.close()
            DatagramSocket()
        }
        receiverThread = Thread(::receiverLoop, "cc-receiver").apply { start() }
    }

    /** Release everything. The instance can't be reopened. */
    fun close() {
        disconnect()
        running = false
        socket?.close()
        receiverThread = null
        try { multicastLock?.release() } catch (_: Exception) {}
    }

    /** Start streaming input to [serverIp]. */
    fun connect(serverIp: String) {
        disconnect()
        val t = Thread({ senderLoop(serverIp) }, "cc-sender")
        senderThread = t
        t.start()
    }

    /** Stop sending input; keep listening for the beacon. */
    fun disconnect() {
        val t = senderThread
        senderThread = null
        t?.interrupt()
    }

    private fun senderLoop(serverIp: String) {
        val me = Thread.currentThread()
        try {
            val address = InetAddress.getByName(serverIp)
            val socket = socket ?: return
            while (running && senderThread === me) {
                val bytes = Protocol.encodeInput(inputProvider())
                socket.send(DatagramPacket(bytes, bytes.size, address, Protocol.INPUT_PORT))
                // ~100Hz: the steering estimate updates at 200Hz, so sending at
                // 60 was throwing away half of it for 10 bytes a packet.
                Thread.sleep(10)
            }
        } catch (_: InterruptedException) {
        } catch (e: Exception) {
            if (running) Log.w("CorsaConnect", "sender stopped: ${e.message}")
        }
    }

    private fun receiverLoop() {
        try {
            val socket = socket ?: return
            val buf = ByteArray(128)
            while (running) {
                val packet = DatagramPacket(buf, buf.size)
                socket.receive(packet)
                if (Protocol.isBeacon(packet.data, packet.length)) {
                    packet.address?.hostAddress?.let(onServerFound)
                } else if (senderThread != null) {
                    Protocol.decodeTelemetry(packet.data, packet.length)?.let(onTelemetry)
                }
            }
        } catch (e: Exception) {
            if (running) Log.w("CorsaConnect", "receiver stopped: ${e.message}")
        }
    }
}
