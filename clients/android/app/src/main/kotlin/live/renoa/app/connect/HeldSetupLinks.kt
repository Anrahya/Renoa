package live.renoa.app.connect

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import live.renoa.rcp.NoticeKey
import live.renoa.rcp.RcpSurface
import live.renoa.rcp.SetupLink
import live.renoa.rcp.UnreadableNotice
import kotlin.uuid.Uuid

/** A setup link waiting for the owner, under the notice that carried it. */
data class HeldLink(val noticeId: Uuid, val link: SetupLink)

/**
 * The setup links Renoa holds for this phone, sealed to its notice [key] as device [deviceId]. On
 * each connection Renoa accepts, [connected] registers the key and reads them; [read] reads them
 * again. A notice that cannot be opened here never will be, so it is acknowledged away; one that
 * expired is not shown. [done] acknowledges a link the owner acted on, which deletes it everywhere.
 */
class HeldSetupLinks(
    private val surface: RcpSurface,
    private val deviceId: Uuid,
    private val key: suspend () -> NoticeKey,
    private val now: () -> Long = System::currentTimeMillis,
) {
    private val held = MutableStateFlow<List<HeldLink>>(emptyList())
    private val reading = Mutex()

    /** Soonest to expire first, as Renoa lists them. */
    val links: StateFlow<List<HeldLink>> = held.asStateFlow()

    suspend fun connected() {
        surface.setNoticeKey(key().publicKey)
        read()
    }

    suspend fun read() =
        reading.withLock {
            val key = key()
            val opened =
                surface.listNotices().mapNotNull { notice ->
                    try {
                        HeldLink(notice.noticeId, key.open(notice, deviceId))
                    } catch (unreadable: UnreadableNotice) {
                        surface.acknowledgeNotice(notice.noticeId)
                        null
                    }
                }
            held.value = opened.filter { it.link.expiresAtMs > now() }
        }

    suspend fun done(noticeId: Uuid) {
        surface.acknowledgeNotice(noticeId)
        held.update { links -> links.filterNot { it.noticeId == noticeId } }
    }
}
