package live.renoa.app.connect

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlin.uuid.Uuid

/**
 * The first setup link waiting for the owner, over whatever screen is open, since the agent that
 * asked may not be the one on screen. Connect opens the link and puts it away; Not now puts it away.
 * The next one follows.
 */
@Composable
fun SetupLinkCards(links: HeldSetupLinks, modifier: Modifier = Modifier) {
    val held by links.links.collectAsState()
    val uri = LocalUriHandler.current
    val scope = rememberCoroutineScope()
    var problem by remember { mutableStateOf<Pair<Uuid, String>?>(null) }
    val first = held.firstOrNull()
    // Kept while the card leaves, so it slides away showing what it held.
    var leaving by remember { mutableStateOf(first) }
    if (first != null) leaving = first
    fun putAway(noticeId: Uuid) {
        scope.launch {
            try {
                links.done(noticeId)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                problem = noticeId to "Could not reach Renoa. Check your connection and try again."
            }
        }
    }
    AnimatedVisibility(
        first != null,
        modifier.statusBarsPadding().padding(12.dp),
        enter = slideInVertically { -it } + fadeIn(),
        exit = slideOutVertically { -it } + fadeOut(),
    ) {
        val shown = leaving ?: return@AnimatedVisibility
        ConnectCard(
            shown.link,
            onConnect = {
                val opened = runCatching { uri.openUri(shown.link.url) }.isSuccess
                if (opened) putAway(shown.noticeId) else problem = shown.noticeId to "This phone has no browser to open the link in."
            },
            onNotNow = { putAway(shown.noticeId) },
            problem = problem?.takeIf { it.first == shown.noticeId }?.second,
        )
    }
}
