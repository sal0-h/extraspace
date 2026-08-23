package io.github.tymonoman.extraspace

import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.view.View
import android.widget.ImageView
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * Draws the host cursor above the decoded video.
 *
 * Position updates only move the view. Sprite uploads happen only when mutter
 * sends a new shape.
 */
class CursorOverlay(
    private val videoView: View,
    private val cursorView: ImageView,
) {
    private val main = Handler(Looper.getMainLooper())
    private val latest = AtomicReference<CursorUpdate?>()
    private val posted = AtomicBoolean(false)

    private var streamWidth = 0
    private var streamHeight = 0
    private var hotX = 0
    private var hotY = 0
    private var posX = 0
    private var posY = 0
    private var hasPosition = false
    private var bitmap: Bitmap? = null
    private var argbScratch: IntArray = IntArray(0)
    private var lastW = 0
    private var lastH = 0

    init {
        cursorView.isClickable = false
        cursorView.isFocusable = false
        cursorView.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
        videoView.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ ->
            if (hasPosition && bitmap != null) place(posX, posY)
        }
    }

    fun setStreamSize(width: Int, height: Int) {
        streamWidth = width
        streamHeight = height
        if (hasPosition && bitmap != null) place(posX, posY)
    }

    fun submit(update: CursorUpdate) {
        latest.set(update)
        if (posted.compareAndSet(false, true)) {
            main.post { flush() }
        }
    }

    fun hide() {
        submit(CursorUpdate.hide())
    }

    fun detach() {
        main.removeCallbacksAndMessages(null)
        posted.set(false)
        cursorView.visibility = View.GONE
        cursorView.setImageDrawable(null)
        bitmap?.recycle()
        bitmap = null
    }

    private fun flush() {
        posted.set(false)
        val update = latest.get() ?: return
        apply(update)
        if (latest.get() !== update && posted.compareAndSet(false, true)) {
            main.post { flush() }
        }
    }

    private fun apply(update: CursorUpdate) {
        if (!update.visible) {
            hasPosition = false
            cursorView.visibility = View.GONE
            return
        }
        if (update.hasHotspot) {
            hotX = update.hotX
            hotY = update.hotY
        }
        update.bitmap?.let { pixels ->
            if (update.bitmapWidth <= 0 || update.bitmapHeight <= 0) return@let
            installBitmap(pixels, update.bitmapWidth, update.bitmapHeight)
        }
        if (update.hasPosition) {
            hasPosition = true
            posX = update.x
            posY = update.y
        }
        if (!hasPosition || bitmap == null || !place(posX, posY)) {
            cursorView.visibility = View.GONE
        }
    }

    private fun installBitmap(bgra: ByteArray, width: Int, height: Int) {
        val count = width * height
        if (argbScratch.size < count) {
            argbScratch = IntArray(count)
        }
        var src = 0
        var i = 0
        while (i < count) {
            val b = bgra[src].toInt() and 0xff
            val g = bgra[src + 1].toInt() and 0xff
            val r = bgra[src + 2].toInt() and 0xff
            val a = bgra[src + 3].toInt() and 0xff
            argbScratch[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
            src += 4
            i++
        }
        val existing = bitmap
        val target = if (existing != null && existing.width == width && existing.height == height) {
            existing
        } else {
            existing?.recycle()
            lastW = 0
            lastH = 0
            Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).also { bitmap = it }
        }
        target.setPixels(argbScratch, 0, width, 0, 0, width, height)
        cursorView.setImageBitmap(target)
    }

    private fun place(streamX: Int, streamY: Int): Boolean {
        val viewW = videoView.width.toFloat()
        val viewH = videoView.height.toFloat()
        val bmp = bitmap
        if (viewW <= 0f || viewH <= 0f || streamWidth <= 0 || streamHeight <= 0 || bmp == null) {
            return false
        }
        val scale = minOf(viewW / streamWidth, viewH / streamHeight)
        val offsetX = (viewW - streamWidth * scale) / 2f
        val offsetY = (viewH - streamHeight * scale) / 2f
        val x = offsetX + (streamX - hotX) * scale
        val y = offsetY + (streamY - hotY) * scale
        val w = (bmp.width * scale).toInt().coerceAtLeast(1)
        val h = (bmp.height * scale).toInt().coerceAtLeast(1)
        if (w != lastW || h != lastH) {
            lastW = w
            lastH = h
            val lp = cursorView.layoutParams
            lp.width = w
            lp.height = h
            cursorView.layoutParams = lp
        }
        cursorView.translationX = x
        cursorView.translationY = y
        cursorView.visibility = View.VISIBLE
        return true
    }
}
