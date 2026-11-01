package com.kroblus.lanrelay

import kotlinx.coroutines.flow.MutableStateFlow
import org.json.JSONObject
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.SocketTimeoutException
import java.util.UUID

data class NearbyDesktop(val id: String, val name: String, val origin: String, val seenAt: Long, val invited: Boolean = false)

internal object DiscoveryProtocol {
    const val PORT = 4319
    const val MAX_BYTES = 1200
    const val TTL_MILLIS = 15_000L
    fun probe(peerId: String, name: String): ByteArray = JSONObject().put("magic", "lanrelay").put("v", 1)
        .put("kind", "probe").put("peer_id", peerId).put("name", name.take(80).ifBlank { "我的手机" })
        .put("role", "android").put("port", 0).toString().toByteArray(Charsets.UTF_8)

    fun parse(bytes: ByteArray, address: InetAddress, now: Long): NearbyDesktop? = runCatching {
        require(address is Inet4Address && LinkPolicy.privateAddress(address))
        require(bytes.size in 1..MAX_BYTES)
        val j = JSONObject(String(bytes, Charsets.UTF_8))
        require(j.keys().asSequence().toSet() == setOf("magic", "v", "kind", "peer_id", "name", "role", "port"))
        require(j.getString("magic") == "lanrelay" && j.getInt("v") == 1 && j.getString("role") == "desktop")
        val kind = j.getString("kind")
        require(kind == "announce" || kind == "connect")
        val id = j.getString("peer_id")
        require(UUID.fromString(id).toString() == id)
        val name = j.getString("name")
        require(name.isNotBlank() && name.length <= 80 && name.none(Char::isISOControl))
        val port = j.getInt("port")
        require(port in 1..65535)
        // The packet supplies no trusted URL. Only its private source address is used.
        NearbyDesktop(id, name, LinkPolicy.origin("http://${address.hostAddress}:$port"), now, kind == "connect")
    }.getOrNull()
}

/** Foreground-only, bounded discovery. Advertisements are hints, never credentials. */
class LanDiscovery(private val peerId: String, private val name: () -> String) : AutoCloseable {
    val peers = MutableStateFlow<List<NearbyDesktop>>(emptyList())
    val invitation = MutableStateFlow<NearbyDesktop?>(null)
    val active = MutableStateFlow(false)
    val error = MutableStateFlow("")
    @Volatile private var worker: Thread? = null
    @Volatile private var socket: DatagramSocket? = null

    @Synchronized fun start() {
        if (worker?.isAlive == true) return
        val thread = Thread({ runDiscovery() }, "lanrelay-discovery").apply { isDaemon = true }
        worker = thread
        thread.start()
    }
    private fun runDiscovery() {
        val current = Thread.currentThread()
        val discovered = linkedMapOf<String, NearbyDesktop>()
        val lastInvitations = mutableMapOf<String, Long>()
        try {
            DatagramSocket(null).use { datagram ->
                datagram.broadcast = true
                datagram.bind(InetSocketAddress(0))
                datagram.soTimeout = 500
                synchronized(this) {
                    if (worker !== current) return
                    socket = datagram
                    active.value = true
                    error.value = ""
                }
                var nextProbe = 0L
                while (worker === current && !current.isInterrupted) {
                    val now = elapsedMillis()
                    if (now >= nextProbe) {
                        val payload = DiscoveryProtocol.probe(peerId, name())
                        val broadcasts = linkedSetOf(InetAddress.getByName("255.255.255.255"))
                        NetworkInterface.getNetworkInterfaces().toList().filter { it.isUp && !it.isLoopback }.forEach { iface ->
                            iface.interfaceAddresses.mapNotNull { it.broadcast }.forEach(broadcasts::add)
                        }
                        broadcasts.forEach { address -> runCatching { datagram.send(DatagramPacket(payload, payload.size, address, DiscoveryProtocol.PORT)) } }
                        nextProbe = now + 5000
                    }
                    discovered.entries.removeAll { now - it.value.seenAt >= DiscoveryProtocol.TTL_MILLIS }
                    lastInvitations.entries.removeAll { now - it.value > 30_000 }
                    peers.value = discovered.values.sortedBy { it.name }.toList()
                    val packet = DatagramPacket(ByteArray(DiscoveryProtocol.MAX_BYTES + 1), DiscoveryProtocol.MAX_BYTES + 1)
                    try { datagram.receive(packet) } catch (_: SocketTimeoutException) { continue }
                    val peer = DiscoveryProtocol.parse(packet.data.copyOf(packet.length), packet.address, elapsedMillis()) ?: continue
                    if (peer.id == peerId || (peer.id !in discovered && discovered.size >= 64)) continue
                    discovered[peer.id] = peer.copy(invited = false)
                    if (peer.invited && invitation.value == null && peer.id !in lastInvitations) {
                        lastInvitations[peer.id] = elapsedMillis()
                        invitation.value = peer
                    }
                }
            }
        } catch (_: Exception) {
            if (worker === current) error.value = "暂时无法发现设备，请确认局域网权限与 Wi-Fi；也可展开手动连接。"
        } finally {
            synchronized(this) { if (worker === current) { worker = null; socket = null; active.value = false } }
        }
    }
    @Synchronized override fun close() {
        val current = worker
        worker = null
        socket?.close()
        socket = null
        current?.interrupt()
        peers.value = emptyList()
        invitation.value = null
        active.value = false
    }
}
