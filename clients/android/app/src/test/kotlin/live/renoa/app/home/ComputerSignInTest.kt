package live.renoa.app.home

import live.renoa.app.agents.Problem
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ComputerSignInTest {
    @Test
    fun aCodeIsWholeOnlyWhenRenoaCouldReadItAsOne() {
        for (typed in listOf("7K3MQX9D", "7k3m-qx9d", "7K3M QX9D", "O1L1OIL0")) {
            assertTrue(isComputerCode(typed), typed)
        }
        for (typed in listOf("", "7K3MQX9", "7K3MQX9DA", "UUUUUUUU", "7K3M_QX9D", "7K3MQX9É", "７K3MQX9D")) {
            assertFalse(isComputerCode(typed), typed)
        }
    }

    @Test
    fun aRefusedApprovalSaysTheWaitThatTooManyWrongCodesCost() {
        assertTrue("wait five minutes" in computerProblem(Problem.Conflict))
    }
}
