package live.renoa.app.agents

/** Why a name or purpose cannot be sent. The Host decides in the end; these catch it before the round trip. */
enum class FieldProblem { Blank, TooLong, ControlCharacter }

/**
 * The Host's rules for what the owner types: a name is 1 to 512 bytes once trimmed; a purpose is
 * optional, at most 1,024 bytes, and holds no control characters except newline.
 */
object AgentRules {
    const val MAX_NAME_BYTES = 512
    const val MAX_PURPOSE_BYTES = 1024

    fun checkName(raw: String): FieldProblem? {
        val name = raw.trim()
        return when {
            name.isEmpty() -> FieldProblem.Blank
            name.toByteArray(Charsets.UTF_8).size > MAX_NAME_BYTES -> FieldProblem.TooLong
            else -> null
        }
    }

    fun checkPurpose(raw: String): FieldProblem? {
        val purpose = raw.trim()
        return when {
            purpose.toByteArray(Charsets.UTF_8).size > MAX_PURPOSE_BYTES -> FieldProblem.TooLong
            purpose.any { it.isISOControl() && it != '\n' } -> FieldProblem.ControlCharacter
            else -> null
        }
    }
}
