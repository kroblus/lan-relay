package com.kroblus.lanrelay

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.lightColorScheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

internal object RelayColors {
    val Background = Color(0xFFF7F8F4)
    val Sage = Color(0xFF537762)
    val SageLight = Color(0xFFE8EFE7)
    val Ink = Color(0xFF24342B)
    val Muted = Color(0xFF5F6D64)
    val Line = Color(0xFFE4E9E2)
    val Lavender = Color(0xFFEEEAF4)
    val Apricot = Color(0xFFF6E5D8)
    val LilacInk = Color(0xFF7D6B96)
}

private val RelayType = Typography(
    headlineLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 30.sp, lineHeight = 40.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.5).sp),
    headlineMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 27.sp, lineHeight = 36.sp, fontWeight = FontWeight.SemiBold),
    headlineSmall = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 23.sp, lineHeight = 32.sp, fontWeight = FontWeight.SemiBold),
    titleLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 20.sp, lineHeight = 28.sp, fontWeight = FontWeight.SemiBold),
    titleMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 17.sp, lineHeight = 25.sp, fontWeight = FontWeight.Medium),
    titleSmall = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 15.sp, lineHeight = 22.sp, fontWeight = FontWeight.Medium),
    bodyLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 16.sp, lineHeight = 26.sp),
    bodyMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 16.sp, lineHeight = 25.sp),
    bodySmall = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 13.sp, lineHeight = 20.sp),
    labelLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium),
    labelMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 12.sp, lineHeight = 18.sp, fontWeight = FontWeight.Medium),
    labelSmall = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 11.sp, lineHeight = 16.sp)
)

@Composable
internal fun RelayTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = lightColorScheme(
            primary = RelayColors.Sage, onPrimary = Color.White,
            primaryContainer = RelayColors.SageLight, onPrimaryContainer = Color(0xFF365140),
            secondary = RelayColors.LilacInk, onSecondary = Color.White,
            secondaryContainer = RelayColors.Lavender, onSecondaryContainer = Color(0xFF584B67),
            tertiary = Color(0xFFAA7758), onTertiary = Color.White,
            tertiaryContainer = RelayColors.Apricot, onTertiaryContainer = Color(0xFF684A35),
            background = RelayColors.Background, onBackground = RelayColors.Ink,
            surface = Color.White, onSurface = RelayColors.Ink,
            surfaceVariant = RelayColors.SageLight, onSurfaceVariant = RelayColors.Muted,
            surfaceContainer = RelayColors.Background, surfaceContainerLow = Color.White,
            surfaceContainerHigh = Color(0xFFF0F3ED), surfaceContainerHighest = Color(0xFFEBEFE7),
            outline = Color(0xFFA5B2A7), outlineVariant = RelayColors.Line,
            error = Color(0xFFB05252), errorContainer = Color(0xFFFBEDED), onErrorContainer = Color(0xFF733333)
        ),
        typography = RelayType,
        shapes = Shapes(small = RoundedCornerShape(12.dp), medium = RoundedCornerShape(18.dp),
            large = RoundedCornerShape(24.dp), extraLarge = RoundedCornerShape(28.dp)),
        content = content
    )
}
