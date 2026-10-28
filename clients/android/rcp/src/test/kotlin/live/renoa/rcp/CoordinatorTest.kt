package live.renoa.rcp

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlin.test.AfterTest
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/** Runs a suspending test body on real threads with a ceiling, since the sockets are real. */
fun eventually(block: suspend () -> Unit) = runBlocking(Dispatchers.Default) { withTimeout(15.seconds) { block() } }

/** One fake coordinator per test, and every client a test makes is closed with it. */
abstract class CoordinatorTest {
    protected val coordinator = FakeCoordinator()
    protected val state = InMemoryRcpState()
    private val clients = mutableListOf<RcpSurface>()

    @AfterTest
    fun tearDown() {
        clients.forEach { it.close() }
        coordinator.close()
    }

    protected fun client(
        authentication: Authentication = Authentication.Device(coordinator.credentials),
        rcpState: RcpState = state,
        newCommandId: () -> Uuid = { Uuid.random() },
    ): RcpSurface =
        WebSocketRcpSurface(coordinator.endpoint, authentication, rcpState, newCommandId = newCommandId).also { clients += it }
}
