package live.renoa.app.home

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.MemoryNote
import live.renoa.app.agents.clockTime
import live.renoa.design.CardSection
import live.renoa.design.Palette
import live.renoa.design.Type
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.format.DateTimeFormatter

/** What the agents know about the owner: the profile they all read, then the newest notes they keep. Read only. */
@Composable
internal fun MemoryPage(state: HomeUiState, onRetry: () -> Unit) {
    if (HomePage.Memory in state.unread) return Unread("What your agents remember could not be read.", onRetry)
    val memory = state.memory ?: return LoadingPage("Reading what your agents remember…")
    val about = memory.profile.lines().map { it.trim() }.filter { it.isNotEmpty() }
    val notes = memory.notes.notes
    if (about.isEmpty() && notes.isEmpty()) return Empty("Nothing yet. Tell an agent about yourself, and what it keeps shows here.")
    Column(verticalArrangement = Arrangement.spacedBy(24.dp)) {
        if (about.isNotEmpty()) {
            CardSection(title = "About you") {
                row { Text(about.joinToString("\n"), style = Type.subheadline, color = Palette.text) }
            }
        }
        if (notes.isNotEmpty()) {
            // Corrected notes and those past the newest the Host sends are kept, not shown.
            val footer = "Some notes aren't shown here. Ask about them in a chat.".takeIf { memory.notes.total > notes.size }
            CardSection(title = "Notes", footer = footer) {
                for (note in notes) row { Note(note) }
            }
        }
    }
}

@Composable
private fun Note(note: MemoryNote) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(note.text, style = Type.subheadline, color = Palette.text)
        val about = listOfNotNull(note.author, day(note.happened ?: note.written), note.due?.let(::due)?.let { "Reminds you $it" })
        Text(about.joinToString(" · "), style = Type.footnote, color = Palette.secondary)
    }
}

/** A day as "27 Oct", or nothing for one that does not read. */
private fun day(date: String): String? = runCatching { LocalDate.parse(date).format(DateTimeFormatter.ofPattern("d MMM")) }.getOrNull()

/** A reminder time as "9 Nov, 9:00 AM", or nothing for one that does not read. */
private fun due(at: String): String? =
    runCatching { LocalDateTime.parse(at).let { "${it.format(DateTimeFormatter.ofPattern("d MMM"))}, ${clockTime(it.toLocalTime())}" } }.getOrNull()
