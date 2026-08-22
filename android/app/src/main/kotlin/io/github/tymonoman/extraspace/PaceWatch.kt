package io.github.tymonoman.extraspace

import android.util.Log

/**
 * Logs only suspicious inter-event gaps so logcat stays readable at 60 fps.
 */
class PaceWatch(private val stage: String) {
    private var lastNs = 0L

    fun observe(extra: String = "") {
        val now = System.nanoTime()
        if (lastNs != 0L) {
            val dtMs = (now - lastNs) / 1_000_000L
            if (dtMs >= GAP_MS) {
                Log.w(TAG, "pacing gap stage=$stage dt_ms=$dtMs $extra")
            }
        }
        lastNs = now
    }

    private companion object {
        const val TAG = "extraspace"
        const val GAP_MS = 50L
    }
}
