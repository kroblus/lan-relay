package com.kroblus.lanrelay

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.unit.dp

internal enum class RelayGlyph { Send, Transfer, Devices, Scan, File, Text, Image, Video, Apk, Up, Down, Check, Chevron, Plus, Close, Refresh, More, Copy, Pause, Play, Folder, Info, Edit }

/** Small vector line icons; no bitmap decoding or large icon font dependency. */
@Composable
internal fun RelayIcon(glyph: RelayGlyph, modifier: Modifier = Modifier, tint: Color = LocalContentColor.current) {
    Canvas(modifier.size(24.dp)) {
        scale(size.width / 24f, size.height / 24f, pivot = Offset.Zero) {
            fun line(x1: Float, y1: Float, x2: Float, y2: Float) =
                drawLine(tint, Offset(x1, y1), Offset(x2, y2), 1.7f, StrokeCap.Round)
            fun box(x: Float, y: Float, w: Float, h: Float, radius: Float = 2f) =
                drawRoundRect(tint, Offset(x, y), Size(w, h), CornerRadius(radius), style = Stroke(1.7f))
            fun dot(x: Float, y: Float, radius: Float = 1f) = drawCircle(tint, radius, Offset(x, y))
            when (glyph) {
                RelayGlyph.Send -> {
                    val path = Path().apply { moveTo(3f, 10f); lineTo(21f, 3f); lineTo(14f, 21f); lineTo(10f, 14f); close() }
                    drawPath(path, tint, style = Stroke(1.7f))
                    line(10f, 14f, 21f, 3f)
                }
                RelayGlyph.Transfer -> { line(5f, 7f, 19f, 7f); line(15f, 3f, 19f, 7f); line(15f, 11f, 19f, 7f); line(19f, 17f, 5f, 17f); line(9f, 13f, 5f, 17f); line(9f, 21f, 5f, 17f) }
                RelayGlyph.Devices -> { box(2f, 4f, 16f, 12f); line(7f, 20f, 13f, 20f); line(10f, 16f, 10f, 20f); drawRoundRect(Color.White, Offset(15f, 10f), Size(7f, 12f), CornerRadius(1.8f)); box(15f, 10f, 7f, 12f, 1.8f); dot(18.5f, 19.5f, .6f) }
                RelayGlyph.Scan -> { for (p in listOf(Offset(3f, 3f), Offset(16f, 3f), Offset(3f, 16f), Offset(16f, 16f))) { box(p.x, p.y, 5f, 5f, .6f) }; line(11f, 3f, 11f, 8f); line(3f, 11f, 8f, 11f); line(13f, 11f, 21f, 11f); line(11f, 14f, 11f, 21f) }
                RelayGlyph.File, RelayGlyph.Apk -> { val path = Path().apply { moveTo(6f, 2f); lineTo(14f, 2f); lineTo(20f, 8f); lineTo(20f, 22f); lineTo(6f, 22f); close() }; drawPath(path, tint, style = Stroke(1.6f)); line(14f, 2f, 14f, 8f); line(14f, 8f, 20f, 8f); if (glyph == RelayGlyph.Apk) { box(9f, 12f, 7f, 5f, 1f); line(10f, 10f, 11f, 12f); line(15f, 10f, 14f, 12f); dot(11f, 14f, .5f); dot(14f, 14f, .5f) } else { line(9f, 13f, 16f, 13f); line(9f, 17f, 14f, 17f) } }
                RelayGlyph.Text -> { box(3f, 3f, 18f, 18f, 4f); line(7f, 8f, 17f, 8f); line(7f, 12f, 17f, 12f); line(7f, 16f, 13f, 16f) }
                RelayGlyph.Image -> { box(3f, 3f, 18f, 18f, 3f); dot(8f, 8f, 1.8f); line(4f, 17f, 10f, 11f); line(10f, 11f, 15f, 16f); line(15f, 16f, 19f, 12f); line(19f, 12f, 21f, 14f) }
                RelayGlyph.Video -> { box(2f, 5f, 14f, 14f, 3f); line(16f, 10f, 22f, 7f); line(22f, 7f, 22f, 17f); line(22f, 17f, 16f, 14f) }
                RelayGlyph.Up, RelayGlyph.Down -> { val up = glyph == RelayGlyph.Up; val a = if (up) 4f else 20f; val b = if (up) 9f else 15f; line(12f, 4f, 12f, 20f); line(7f, b, 12f, a); line(17f, b, 12f, a) }
                RelayGlyph.Check -> { line(5f, 12f, 10f, 17f); line(10f, 17f, 20f, 6f) }
                RelayGlyph.Chevron -> { line(9f, 5f, 16f, 12f); line(16f, 12f, 9f, 19f) }
                RelayGlyph.Plus -> { line(12f, 4f, 12f, 20f); line(4f, 12f, 20f, 12f) }
                RelayGlyph.Close -> { line(6f, 6f, 18f, 18f); line(6f, 18f, 18f, 6f) }
                RelayGlyph.Refresh -> { drawArc(tint, 40f, 290f, false, Offset(4f, 4f), Size(16f, 16f), style = Stroke(1.7f, cap = StrokeCap.Round)); line(20f, 5f, 20f, 11f); line(14f, 11f, 20f, 11f) }
                RelayGlyph.More -> { dot(5f, 12f, 1.4f); dot(12f, 12f, 1.4f); dot(19f, 12f, 1.4f) }
                RelayGlyph.Copy -> { box(8f, 7f, 12f, 14f); line(4f, 17f, 4f, 3f); line(4f, 3f, 15f, 3f) }
                RelayGlyph.Pause -> { line(8f, 5f, 8f, 19f); line(16f, 5f, 16f, 19f) }
                RelayGlyph.Play -> { val p = Path().apply { moveTo(7f, 4f); lineTo(20f, 12f); lineTo(7f, 20f); close() }; drawPath(p, tint, style = Stroke(1.7f)) }
                RelayGlyph.Folder -> { val p = Path().apply { moveTo(3f, 6f); lineTo(10f, 6f); lineTo(12f, 9f); lineTo(21f, 9f); lineTo(21f, 20f); lineTo(3f, 20f); close() }; drawPath(p, tint, style = Stroke(1.7f)); line(3f, 11f, 21f, 11f) }
                RelayGlyph.Info -> { drawCircle(tint, 9f, Offset(12f, 12f), style = Stroke(1.7f)); dot(12f, 7f); line(12f, 11f, 12f, 17f) }
                RelayGlyph.Edit -> { line(5f, 17f, 17f, 5f); line(5f, 17f, 4f, 21f); line(4f, 21f, 8f, 20f); line(8f, 20f, 20f, 8f); line(17f, 5f, 20f, 8f); line(15f, 7f, 18f, 10f) }
            }
        }
    }
}
