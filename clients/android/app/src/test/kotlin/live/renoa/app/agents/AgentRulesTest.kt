package live.renoa.app.agents

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class AgentRulesTest {
    @Test
    fun aNameNeedsAtLeastOneCharacterOnceTrimmed() {
        assertEquals(FieldProblem.Blank, AgentRules.checkName(""))
        assertEquals(FieldProblem.Blank, AgentRules.checkName("   \n"))
        assertNull(AgentRules.checkName("  Tifa "))
    }

    @Test
    fun aNameIsAtMostFiveHundredAndTwelveBytesAndMultiByteCharactersCountInFull() {
        assertNull(AgentRules.checkName("a".repeat(512)))
        assertEquals(FieldProblem.TooLong, AgentRules.checkName("a".repeat(513)))
        assertNull(AgentRules.checkName("é".repeat(256)))
        assertEquals(FieldProblem.TooLong, AgentRules.checkName("é".repeat(257)))
    }

    @Test
    fun aPurposeIsOptionalTextOfAtMostAThousandAndTwentyFourBytes() {
        assertNull(AgentRules.checkPurpose(""))
        assertNull(AgentRules.checkPurpose("Keep the house running.\nGroceries, bills."))
        assertNull(AgentRules.checkPurpose("a".repeat(1024)))
        assertEquals(FieldProblem.TooLong, AgentRules.checkPurpose("a".repeat(1025)))
    }

    @Test
    fun aPurposeHoldsNoControlCharactersExceptNewline() {
        assertEquals(FieldProblem.ControlCharacter, AgentRules.checkPurpose("bell\u0007"))
        assertEquals(FieldProblem.ControlCharacter, AgentRules.checkPurpose("tab\there"))
        assertEquals(FieldProblem.ControlCharacter, AgentRules.checkPurpose("nul\u0000"))
    }
}
