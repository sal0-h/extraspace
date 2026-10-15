package io.github.tymonoman.extraspace

import android.annotation.SuppressLint
import android.graphics.SurfaceTexture
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.MotionEvent
import android.view.Surface
import android.view.TextureView
import android.view.View
import android.view.WindowManager
import android.widget.ImageView
import android.widget.TextView
import android.widget.Button
import android.widget.EditText
import android.app.AlertDialog
import android.text.InputType
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import androidx.activity.ComponentActivity
import androidx.activity.addCallback
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/**
 * The screen the tablet actually shows: a full-bleed [TextureView] fed by
 * [VideoDecoder], with touches forwarded back to the host.
 *
 * Deliberately not a Compose UI. Everything here is one surface and one gesture
 * stream, and Compose would add a recomposition layer between the touch event and
 * the socket for no benefit.
 *
 * Video is a TextureView so the cursor ImageView can composite on top.
 * A SurfaceView is a separate plane; on Samsung the decoder sits above any
 * in-window overlay, which made the pointer invisible.
 */
class MirrorActivity : ComponentActivity(), ConnectionManager.Callbacks, WirelessClient.Callbacks {

    private lateinit var textureView: TextureView
    private lateinit var statusView: TextView
    private lateinit var cursorOverlay: CursorOverlay
    @Volatile private var decoder: VideoDecoder? = null
    private var decoderSurface: Surface? = null
    private var connection: ConnectionManager? = null
    private var camera: CameraSource? = null
    private lateinit var wireless: WirelessClient
    private lateinit var pairingControls: View
    @Volatile private var connectionIsLive = false
    private var connectionGeneration = 0
    private var destroyed = false
    private val main = Handler(Looper.getMainLooper())
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        result.contents?.let { contents ->
            runCatching { wireless.pairWithQr(contents) }.onFailure { showStatus(it.message) }
        }
    }

    /** Dimensions of the incoming stream; touches are mapped into this space. */
    private var streamWidth = 0
    private var streamHeight = 0

    private val statsTicker = object : Runnable {
        override fun run() {
            decoder?.let { d ->
                connection?.sendStats(
                    d.queueDepth,
                    d.framesDecoded.get(),
                    d.framesDropped.get(),
                    d.lastFramePtsUs.get(),
                    d.renderedAtUs.get(),
                )
            }
            main.postDelayed(this, STATS_INTERVAL_MS)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        DeviceInfo.load(this)

        // A second monitor that sleeps is not a second monitor.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        goFullscreen()

        setContentView(R.layout.activity_mirror)
        textureView = findViewById(R.id.surface)
        statusView = findViewById(R.id.status)
        pairingControls = findViewById(R.id.pairing_controls)
        wireless = WirelessClient(this, this)
        findViewById<Button>(R.id.pair_computer).setOnClickListener { showPairingOptions() }
        findViewById<Button>(R.id.saved_computer).setOnClickListener { showSavedComputer() }
        wireless.start()
        onBackPressedDispatcher.addCallback(this) { if (connectionIsLive) showSavedComputer() else finish() }
        textureView.isOpaque = true
        cursorOverlay = CursorOverlay(textureView, findViewById<ImageView>(R.id.cursor))

        textureView.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
            override fun onSurfaceTextureAvailable(surface: SurfaceTexture, width: Int, height: Int) {
                attachDecoder(surface)
            }

            override fun onSurfaceTextureSizeChanged(surface: SurfaceTexture, width: Int, height: Int) = Unit

            override fun onSurfaceTextureDestroyed(surface: SurfaceTexture): Boolean {
                stopDecoder()
                return true
            }

            override fun onSurfaceTextureUpdated(surface: SurfaceTexture) = Unit
        }
        textureView.surfaceTexture?.let { attachDecoder(it) }
    }

    private fun attachDecoder(surfaceTexture: SurfaceTexture) {
        stopDecoder()
        if (streamWidth > 0 && streamHeight > 0) {
            surfaceTexture.setDefaultBufferSize(streamWidth, streamHeight)
        }
        decoderSurface = Surface(surfaceTexture)
        decoder = VideoDecoder(decoderSurface!!)
        if (streamWidth > 0 && streamHeight > 0) {
            decoder?.start(streamWidth, streamHeight, null)
            connection?.videoDecoderReady()
        }
        startConnection()
    }

    private fun stopDecoder() {
        decoder?.stop()
        decoder = null
        decoderSurface?.release()
        decoderSurface = null
    }

    private fun startConnection() {
        if (connection != null) return
        showStatus(wireless.computerName?.let { "Looking for $it…\n\nOpen Extraspace on your computer on the same Wi-Fi, or connect a USB cable." }
            ?: getString(R.string.waiting_for_host))
        connection = newConnection().also { it.start() }
        main.removeCallbacks(statsTicker)
        main.postDelayed(statsTicker, STATS_INTERVAL_MS)
    }

    private fun goFullscreen() {
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    /** Safe to call from any thread; hops to the main thread itself. */
    private fun showStatus(text: String?) {
        main.post {
            statusView.text = text ?: ""
            statusView.visibility = if (text == null) View.GONE else View.VISIBLE
        }
    }

    // ------------------------------------------------------- host callbacks
    // All of these arrive on socket threads, so anything touching a view or the
    // decoder is posted to the main thread.
    override fun onVideoConfig(width: Int, height: Int, framerate: Int) {
        main.post {
            streamWidth = width
            streamHeight = height
            textureView.surfaceTexture?.setDefaultBufferSize(width, height)
            cursorOverlay.setStreamSize(width, height)
            decoder?.let {
                it.start(width, height, null)
                connection?.videoDecoderReady()
            }
            Log.i(TAG, "stream configured ${width}x$height @$framerate")
        }
    }

    override fun onVideoFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean) {
        // Called on the video thread; MediaCodec is happy to be driven from here
        // and hopping to the main thread would only add latency.
        if (decoder == null) return
        if (streamWidth == 0) return
        decoder?.decode(data, length, ptsUs, isConfig)
        if (statusView.visibility == View.VISIBLE) showStatus(null)
    }

    override fun onCameraControl(
        enabled: Boolean, cameraId: String, width: Int, height: Int, framerate: Int, bitrateKbps: Int,
    ) {
        main.post {
            camera?.stop()
            camera = if (enabled) {
                CameraSource(this) { data, length, ptsUs, isConfig, isKey ->
                    connection?.sendCameraFrame(data, length, ptsUs, isConfig, isKey)
                }.also { it.start(cameraId, width, height, framerate, bitrateKbps) }
            } else {
                null
            }
        }
    }

    override fun onCursor(update: CursorUpdate) {
        cursorOverlay.submit(update)
    }

    override fun onConnected() {
        connectionIsLive = true
        wireless.setStreaming(true)
        main.post { pairingControls.visibility = View.GONE }
        showStatus(null)
    }

    override fun onDisconnected(reason: String) {
        // The generation guard dispatches this on the main thread. Invalidate
        // that generation now, before another channel can report the same loss.
        if (destroyed) return
        Log.i(TAG, "connection ended; waiting for computer: $reason")
        connectionGeneration++
        connectionIsLive = false
        connection?.close(); connection = null
        camera?.stop(); camera = null
        cursorOverlay.hide()
        stopDecoder(); streamWidth = 0; streamHeight = 0
        textureView.surfaceTexture?.let { attachDecoder(it) }
        pairingControls.visibility = View.VISIBLE
        wireless.setStreaming(false)
    }

    override fun onWirelessStatus(message: String) { if (!connectionIsLive) showStatus(message) }

    override fun onPaired(name: String) { showStatus("Paired with $name. Connecting…") }

    override fun onWirelessConnected(channels: WirelessChannels) {
        main.post {
            if (destroyed || connectionIsLive) { channels.close(); return@post }
            connection?.close(); connection = null
            stopDecoder(); streamWidth = 0; streamHeight = 0
            textureView.surfaceTexture?.let {
                decoderSurface = Surface(it)
                decoder = VideoDecoder(decoderSurface!!)
            }
            connection = newConnection(channels).also { it.start() }
        }
    }

    private fun newConnection(channels: WirelessChannels? = null): ConnectionManager {
        val generation = ++connectionGeneration
        val guarded = object : ConnectionManager.Callbacks by this {
            override fun onConnected() {
                main.post { if (generation == connectionGeneration && !destroyed) this@MirrorActivity.onConnected() }
            }
            override fun onDisconnected(reason: String) {
                main.post { if (generation == connectionGeneration && !destroyed) this@MirrorActivity.onDisconnected(reason) }
            }
        }
        return ConnectionManager(guarded, channels)
    }

    private fun showPairingOptions() {
        AlertDialog.Builder(this).setTitle("Pair computer")
            .setMessage("On your computer, open Extraspace and choose Pair tablet. Keep both devices on the same Wi-Fi.")
            .setPositiveButton("Scan QR code") { _, _ ->
                scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                    .setPrompt("Scan the QR code in Extraspace on your computer")
                    .setBeepEnabled(false).setOrientationLocked(false))
            }
            .setNeutralButton("Enter code") { _, _ -> showCodeEntry() }
            .setNegativeButton("Cancel", null).show()
    }

    private fun showCodeEntry() {
        val input = EditText(this).apply {
            hint = "20 letters and numbers"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            isSingleLine = true
        }
        val dialog = AlertDialog.Builder(this).setTitle("Enter pairing code")
            .setMessage("Enter the code shown in Extraspace on your computer. Dashes are optional.")
            .setView(input).setPositiveButton("Pair", null).setNegativeButton("Cancel", null).create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                runCatching { wireless.pairWithCode(input.text.toString()) }
                    .onSuccess { dialog.dismiss() }.onFailure { input.error = it.message }
            }
        }
        dialog.show()
    }

    private fun showSavedComputer() {
        val name = wireless.computerName
        val dialog = AlertDialog.Builder(this).setTitle(name ?: "No saved computer")
            .setMessage(if (name == null) "Choose Pair computer to save a computer for automatic connection."
                else "Extraspace connects to this computer automatically. Forget it to remove its saved permission.")
            .setNegativeButton("Close", null)
        if (name != null) dialog.setPositiveButton("Forget computer") { _, _ ->
            connectionGeneration++
            connection?.close(); connection = null; connectionIsLive = false
            camera?.stop(); camera = null
            wireless.forget()
            stopDecoder(); streamWidth = 0; streamHeight = 0
            textureView.surfaceTexture?.let { attachDecoder(it) }
        }
        dialog.show()
    }

    // --------------------------------------------------------------- touch
    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        val conn = connection ?: return false
        if (streamWidth == 0 || streamHeight == 0) return false

        // The surface may be letterboxed if the host's monitor aspect ratio does
        // not exactly match the panel, so map through the displayed rectangle
        // rather than assuming the view fills the screen.
        val viewW = textureView.width.toFloat()
        val viewH = textureView.height.toFloat()
        if (viewW <= 0f || viewH <= 0f) return false
        val scale = minOf(viewW / streamWidth, viewH / streamHeight)
        val offsetX = (viewW - streamWidth * scale) / 2f
        val offsetY = (viewH - streamHeight * scale) / 2f

        fun mapX(raw: Float) = ((raw - offsetX) / scale).toDouble().coerceIn(0.0, streamWidth - 1.0)
        fun mapY(raw: Float) = ((raw - offsetY) / scale).toDouble().coerceIn(0.0, streamHeight - 1.0)

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val i = event.actionIndex
                conn.sendTouch(
                    TouchEvent(
                        Protocol.TouchAction.DOWN, event.getPointerId(i),
                        mapX(event.getX(i)), mapY(event.getY(i)),
                    )
                )
            }
            MotionEvent.ACTION_MOVE -> {
                // A MOVE batches every pointer that changed, so emit one per finger.
                for (i in 0 until event.pointerCount) {
                    conn.sendTouch(
                        TouchEvent(
                            Protocol.TouchAction.MOTION, event.getPointerId(i),
                            mapX(event.getX(i)), mapY(event.getY(i)),
                        )
                    )
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_CANCEL -> {
                val i = event.actionIndex
                conn.sendTouch(
                    TouchEvent(
                        Protocol.TouchAction.UP, event.getPointerId(i),
                        mapX(event.getX(i)), mapY(event.getY(i)),
                    )
                )
            }
        }
        return true
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) goFullscreen()
    }

    override fun onDestroy() {
        destroyed = true
        wireless.close()
        main.removeCallbacks(statsTicker)
        camera?.stop()
        connection?.close()
        stopDecoder()
        cursorOverlay.detach()
        super.onDestroy()
    }

    private companion object {
        const val TAG = "extraspace"
        const val STATS_INTERVAL_MS = 500L
    }
}
