package live.renoa.app.connect

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
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

    /** Under the same lock as [read], so a read that listed the link before it was acknowledged cannot show it again. */
    suspend fun done(noticeId: Uuid) =
        reading.withLock {
            surface.acknowledgeNotice(noticeId)
            held.update { links -> links.filterNot { it.noticeId == noticeId } }
        }
}

/** Emits each time an agent that was working stops, as when a setup link ends its turn. */
internal fun Flow<List<AgentSummary>>.turnsEnded(): Flow<Unit> =
    flow {
        var working = emptySet<String>()
        collect { agents ->
            val now = agents.filter { it.state == AgentState.Working }.mapTo(HashSet()) { it.id }
            if ((working - now).isNotEmpty()) emit(Unit)
            working = now
        }
    }
