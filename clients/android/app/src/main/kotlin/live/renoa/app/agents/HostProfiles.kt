package live.renoa.app.agents

import java.util.concurrent.ConcurrentHashMap
import kotlin.uuid.Uuid

/**
 * What the Host says about each of its agents: what it is for and how it looks. Each is read until
 * the Host has answered for it, with it or with no such agent, and not again this launch; any other
 * failure leaves it to be read on the next call to [read].
 */
internal class HostProfiles(private val host: HostAgents) {
    private class Profile {
        @Volatile var purpose: String? = null

        @Volatile var look: Look? = null

        @Volatile var lookRead = false
    }

    private val profiles = ConcurrentHashMap<Uuid, Profile>()

    /** What [agent] is for; null until the Host has said. */
    fun purpose(agent: Uuid): String? = profiles[agent]?.purpose

    /** How [agent] looks; null until the Host has said, or when it has no such agent. */
    fun look(agent: Uuid): Look? = profiles[agent]?.look

    /** Records what [agent] is for, as the Host answered when it made it. */
    fun learned(agent: Uuid, purpose: String) {
        profile(agent).purpose = purpose
    }

    /** Reads what the Host has not answered yet for each of [agents], calling [changed] after each answer. */
    suspend fun read(agents: List<Uuid>, changed: () -> Unit) {
        for (agent in agents) {
            val profile = profile(agent)
            if (profile.purpose == null) {
                profile.purpose =
                    try {
                        host.definition(agent).operational.purpose.orEmpty()
                    } catch (failed: RepositoryException) {
                        if (failed.problem != Problem.NotFound) continue
                        // No such agent: it has no look either.
                        profile.lookRead = true
                        ""
                    }
                changed()
            }
            if (!profile.lookRead) {
                profile.look =
                    try {
                        host.appearance(agent).look
                    } catch (failed: RepositoryException) {
                        if (failed.problem != Problem.NotFound) continue
                        null
                    }
                profile.lookRead = true
                changed()
            }
        }
    }

    private fun profile(agent: Uuid): Profile = profiles.getOrPut(agent) { Profile() }
}
