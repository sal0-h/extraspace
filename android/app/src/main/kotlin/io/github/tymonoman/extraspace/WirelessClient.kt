package io.github.tymonoman.extraspace

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONObject
import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.net.InetSocketAddress
import java.security.KeyStore
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.Mac
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.TrustManager
import javax.net.ssl.X509TrustManager
import kotlin.concurrent.thread

/** The app owns this connection and its trust. No ADB or Developer options. */
class WirelessClient(context: Context, private val callbacks: Callbacks) : Closeable {
    interface Callbacks {
        fun onWirelessStatus(message: String)
        fun onPaired(name: String)
        fun onWirelessConnected(channels: WirelessChannels)
    }

    private data class Endpoint(val host: String, val port: Int)
    private data class PairRequest(val code: String, val fingerprint: String?, val endpoints: List<Endpoint>)
    private val app = context.applicationContext
    private val nsd = app.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val running = AtomicBoolean(false)
    private val discovered = ConcurrentHashMap<String, Endpoint>()
    private val services = ConcurrentHashMap<String, NsdServiceInfo>()
    private val resolving = AtomicBoolean(false)
    private val preferences = app.getSharedPreferences("wireless", Context.MODE_PRIVATE)
    @Volatile private var request: PairRequest? = null
    @Volatile private var paired: JSONObject? = load()
    @Volatile private var streaming = false
    @Volatile private var activeSocket: SSLSocket? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var worker: Thread? = null

    val computerName: String? get() = paired?.optString("name")

    fun start() {
        if (!running.compareAndSet(false, true)) return
        val wifi = app.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock("extraspace-discovery").apply { setReferenceCounted(false); acquire() }
        runCatching { nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discovery) }
        worker = thread(name = "xs-wifi") {
            var refresh = 0
            while (running.get()) {
                val knownServices = services.values.toList()
                if (knownServices.isNotEmpty()) resolve(knownServices[refresh++ % knownServices.size])
                if (!streaming) {
                    val pairing = request
                    if (pairing != null) tryPair(pairing) else paired?.let { tryConnect(it) }
                }
                try { Thread.sleep(2000) } catch (_: InterruptedException) { break }
            }
        }
    }

    fun pairWithQr(contents: String) {
        val json = JSONObject(contents)
        require(json.getInt("extraspace") == 1) { "This is not an Extraspace pairing QR code." }
        val fingerprint = json.getString("fingerprint")
        require(fingerprint.matches(Regex("[0-9a-f]{64}"))) { "Invalid computer identity." }
        val port = json.getInt("port")
        require(port in 1..65535) { "Invalid connection port." }
        val hosts = json.getJSONArray("hosts")
        val endpoints = (0 until hosts.length()).map { Endpoint(hosts.getString(it), port) }
        request = PairRequest(normalizeCode(json.getString("code")), fingerprint, endpoints)
        callbacks.onWirelessStatus("Pairing with your computer…")
    }

    fun pairWithCode(code: String) {
        request = PairRequest(normalizeCode(code), null, emptyList())
        callbacks.onWirelessStatus("Looking for the computer showing that code…")
    }

    fun forget() {
        request = null; paired = null; streaming = false
        preferences.edit().remove("credential").commit()
        runCatching { activeSocket?.close() }
        callbacks.onWirelessStatus("Computer forgotten. Choose Pair computer to connect again.")
    }

    fun setStreaming(value: Boolean) { streaming = value }

    private fun tryPair(pairing: PairRequest) {
        val endpoints = (pairing.endpoints + discovered.values).distinct()
        for (endpoint in endpoints) {
            if (!running.get() || request !== pairing || streaming) return
            try {
                tls(endpoint, pairing.fingerprint).use { socket ->
                    activeSocket = socket
                    val fingerprint = fingerprint(socket)
                    val clientNonce = randomHex(32)
                    write(socket, JSONObject().put("kind", "pair_hello").put("nonce", clientNonce))
                    val challenge = read(socket)
                    require(challenge.getString("kind") == "pair_challenge")
                    val serverNonce = challenge.getString("nonce")
                    require(serverNonce.matches(Regex("[0-9a-f]{64}")))
                    val transcript = "$fingerprint:$clientNonce:$serverNonce"
                    val expected = proof(pairing.code, "server", transcript)
                    require(MessageDigest.isEqual(expected.toByteArray(), challenge.getString("proof").toByteArray())) {
                        "Pairing code did not match. Get a new code from Extraspace on your computer."
                    }
                    // Only after the certificate-bound proof succeeds do we send
                    // a credential. A relay using another TLS certificate fails.
                    val token = randomHex(32)
                    write(socket, JSONObject().put("kind", "pair_finish")
                        .put("proof", proof(pairing.code, "client", transcript)).put("token", token)
                        .put("name", "${Build.MANUFACTURER} ${Build.MODEL}"))
                    val result = read(socket)
                    require(result.getString("kind") == "paired" && result.getString("id") == fingerprint)
                    val credential = JSONObject().put("id", fingerprint).put("fingerprint", fingerprint)
                        .put("name", result.getString("name")).put("token", token)
                        .put("host", endpoint.host).put("port", endpoint.port)
                    save(credential)
                    paired = credential
                    if (request === pairing) request = null
                    callbacks.onPaired(credential.getString("name"))
                    return
                }
            } catch (_: Exception) {
                // Other Extraspace computers may advertise on the same network.
                // None is trusted unless the entered code authenticates its cert.
            } finally { activeSocket = null }
        }
        if (request === pairing) callbacks.onWirelessStatus("Could not pair yet. Keep the pairing window open on your computer and use the same Wi-Fi. Check the code or scan a new QR code.")
    }

    private fun tryConnect(credential: JSONObject) {
        val id = credential.getString("id")
        val cached = Endpoint(credential.getString("host"), credential.getInt("port"))
        val endpoints = listOfNotNull(discovered[id], cached).distinct()
        for (endpoint in endpoints) {
            if (!running.get() || streaming || paired !== credential) return
            val sockets = mutableListOf<SSLSocket>()
            try {
                val session = randomHex(16)
                for (channel in listOf("control", "video", "camera")) {
                    val socket = tls(endpoint, credential.getString("fingerprint"))
                    activeSocket = socket; sockets.add(socket)
                    write(socket, JSONObject().put("kind", "channel").put("channel", channel)
                        .put("session", session).put("token", credential.getString("token")))
                    require(read(socket).getString("kind") == "ready")
                    socket.soTimeout = 0
                }
                if (!running.get() || streaming || paired !== credential) throw IOException("Connection cancelled")
                streaming = true
                if (credential.optString("host") != endpoint.host || credential.optInt("port") != endpoint.port) {
                    credential.put("host", endpoint.host).put("port", endpoint.port)
                    save(credential)
                }
                callbacks.onWirelessConnected(WirelessChannels(sockets[0], sockets[1], sockets[2]))
                return
            } catch (_: Exception) {
                sockets.forEach { runCatching { it.close() } }
            } finally { activeSocket = null }
        }
    }

    private val discovery = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(type: String) = Unit
        override fun onDiscoveryStopped(type: String) = Unit
        override fun onStartDiscoveryFailed(type: String, code: Int) = Unit
        override fun onStopDiscoveryFailed(type: String, code: Int) = Unit
        override fun onServiceFound(info: NsdServiceInfo) { services[info.serviceName] = info; resolve(info) }
        override fun onServiceLost(info: NsdServiceInfo) { services.remove(info.serviceName) }
    }

    @Suppress("DEPRECATION")
    private fun resolve(info: NsdServiceInfo) {
        if (!running.get() || !resolving.compareAndSet(false, true)) return
        nsd.resolveService(info, object : NsdManager.ResolveListener {
            override fun onResolveFailed(service: NsdServiceInfo, code: Int) { resolving.set(false) }
            override fun onServiceResolved(service: NsdServiceInfo) {
                resolving.set(false)
                val id = service.attributes["id"]?.toString(Charsets.UTF_8) ?: return
                val host = service.host?.hostAddress ?: return
                if (id.matches(Regex("[0-9a-f]{64}"))) discovered[id] = Endpoint(host, service.port)
            }
        })
    }

    private fun tls(endpoint: Endpoint, pin: String?): SSLSocket {
        val trust = object : X509TrustManager {
            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
            override fun checkClientTrusted(chain: Array<X509Certificate>, type: String) = throw java.security.cert.CertificateException()
            override fun checkServerTrusted(chain: Array<X509Certificate>, type: String) {
                require(chain.isNotEmpty()) { "No computer identity" }
                if (pin != null) require(sha256(chain[0].encoded) == pin) { "Computer identity changed. Pair again to allow it." }
                // Unpinned certificates are accepted only for the temporary
                // pairing exchange. Its HMAC authenticates this exact cert.
            }
        }
        val context = SSLContext.getInstance("TLSv1.3").apply { init(null, arrayOf<TrustManager>(trust), SecureRandom()) }
        val socket = context.socketFactory.createSocket() as SSLSocket
        try {
            socket.enabledProtocols = arrayOf("TLSv1.3")
            socket.soTimeout = 5000; socket.tcpNoDelay = true
            activeSocket = socket
            socket.connect(InetSocketAddress(endpoint.host, endpoint.port), 2500)
            socket.startHandshake()
            return socket
        } catch (e: Exception) { runCatching { socket.close() }; throw e }
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }

    private fun save(value: JSONObject) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val encrypted = cipher.doFinal(value.toString().toByteArray())
        val encoded = Base64.encodeToString(cipher.iv + encrypted, Base64.NO_WRAP)
        check(preferences.edit().putString("credential", encoded).commit()) { "Could not save paired computer" }
    }

    private fun load(): JSONObject? {
        val encoded = preferences.getString("credential", null) ?: return null
        return runCatching {
            val data = Base64.decode(encoded, Base64.NO_WRAP)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, data.copyOfRange(0, 12))) }
            JSONObject(String(cipher.doFinal(data.copyOfRange(12, data.size))))
        }.getOrNull()
    }

    override fun close() {
        running.set(false); request = null
        runCatching { nsd.stopServiceDiscovery(discovery) }
        runCatching { multicast?.release() }
        runCatching { activeSocket?.close() }
        worker?.interrupt()
    }

    companion object {
        private const val SERVICE_TYPE = "_extraspace._tcp."
        private const val KEY_ALIAS = "extraspace-wifi-credential"
        private fun normalizeCode(value: String): String {
            val code = value.uppercase(java.util.Locale.ROOT).filterNot { it == '-' || it.isWhitespace() }
            require(code.length == 20 && code.all { it in "ABCDEFGHJKLMNPQRSTUVWXYZ23456789" }) { "Enter all 20 letters and numbers shown on your computer." }
            return code
        }
        private fun randomHex(size: Int): String = ByteArray(size).also { SecureRandom().nextBytes(it) }.hex()
        private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it.toInt() and 255) }
        private fun sha256(data: ByteArray) = MessageDigest.getInstance("SHA-256").digest(data).hex()
        private fun fingerprint(socket: SSLSocket) = sha256(socket.session.peerCertificates[0].encoded)
        private fun proof(code: String, role: String, transcript: String): String {
            val mac = Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(code.toByteArray(), "HmacSHA256")) }
            return mac.doFinal("extraspace-pair-$role-v1:$transcript".toByteArray()).hex()
        }
        private fun write(socket: SSLSocket, value: JSONObject) {
            val data = value.toString().toByteArray()
            DataOutputStream(socket.outputStream).apply { writeInt(data.size); write(data); flush() }
        }
        private fun read(socket: SSLSocket): JSONObject {
            val input = DataInputStream(socket.inputStream)
            val size = input.readInt(); require(size in 1..8192) { "Invalid handshake" }
            val data = ByteArray(size); input.readFully(data)
            return JSONObject(String(data))
        }
    }
}

data class WirelessChannels(val control: SSLSocket, val video: SSLSocket, val camera: SSLSocket) : Closeable {
    override fun close() { listOf(control, video, camera).forEach { runCatching { it.close() } } }
}
