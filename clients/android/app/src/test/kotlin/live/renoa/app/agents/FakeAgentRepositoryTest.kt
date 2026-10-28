package live.renoa.app.agents

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class FakeAgentRepositoryTest {
    private val ids = generateSequence(1) { it + 1 }.map { "agent-$it" }.iterator()
    private val oneTwoThree = ReplyScript { _, _ -> Reply.Words("one two three") }

    private fun TestScope.repository(script: ReplyScript = oneTwoThree) =
        FakeAgentRepository(backgroundScope, FakeWorld.demo(), wordDelayMs = 100, script = script, newId = { ids.next() })

    private fun FakeAgentRepository.state(id: String) = agents.value.first { it.id == id }.state

    private suspend fun FakeAgentRepository.chat(id: String) = conversation(id).first()

    // --- chat ---

    @Test
    fun aMessageIsKeptAtOnceThenTheAgentWorksStreamsAndSettles() = runTest {
        val repo = repository()
        repo.send("nagisa", "hello")
        assertEquals(listOf("hello"), repo.chat("nagisa").messages.map { it.text }, "the message is kept before any reply")
        runCurrent()
        assertEquals(AgentState.Working, repo.state("nagisa"))
        assertNull(repo.chat("nagisa").streaming, "it thinks before it speaks")

        advanceTimeBy(750)
        assertEquals("one", repo.chat("nagisa").streaming?.text)
        advanceTimeBy(100)
        assertEquals("one two", repo.chat("nagisa").streaming?.text)

        settle()
        val done = repo.chat("nagisa")
        assertEquals(listOf(Speaker.Owner to "hello", Speaker.Agent to "one two three"), done.messages.map { it.speaker to it.text })
        assertNull(done.streaming, "streamed words are never kept; the whole message replaces them")
        assertEquals(AgentState.Resting, repo.state("nagisa"))
    }

    @Test
    fun stopIsFinalTheReplyNeverArrivesAndTheAgentRests() = runTest {
        val repo = repository()
        repo.send("nagisa", "hello")
        advanceTimeBy(750)
        assertEquals("one", repo.chat("nagisa").streaming?.text)

        repo.stop("nagisa")
        runCurrent()
        assertEquals(AgentState.Resting, repo.state("nagisa"))
        assertNull(repo.chat("nagisa").streaming)

        advanceTimeBy(60_000)
        assertEquals(listOf("hello"), repo.chat("nagisa").messages.map { it.text }, "a stopped agent must not finish its reply later")
        assertEquals(AgentState.Resting, repo.state("nagisa"))
    }

    @Test
    fun stopAlsoDropsMessagesWaitingBehindTheOneBeingAnswered() = runTest {
        val repo = repository()
        repo.send("nagisa", "first")
        repo.send("nagisa", "second")
        advanceTimeBy(750)
        repo.stop("nagisa")
        advanceTimeBy(60_000)
        assertEquals(listOf("first", "second"), repo.chat("nagisa").messages.map { it.text }, "both owner messages are kept, and neither is answered")
    }

    @Test
    fun aMessageDroppedByStopIsNotAnsweredLaterWhenTheOwnerWritesAgain() = runTest {
        val repo = repository()
        repo.send("nagisa", "first")
        repo.send("nagisa", "second")
        advanceTimeBy(750)
        repo.stop("nagisa")
        repo.send("nagisa", "third")
        settle()
        val chat = repo.chat("nagisa").messages
        assertEquals(listOf("first", "second", "third"), chat.filter { it.speaker == Speaker.Owner }.map { it.text })
        assertEquals(1, chat.count { it.speaker == Speaker.Agent }, "only the message sent after the stop is answered")
    }

    @Test
    fun anAgentAlreadyWorkingWithNoTurnOfOursStillStops() = runTest {
        val repo = repository()
        assertEquals(AgentState.Working, repo.state("renoa"))
        repo.stop("renoa")
        assertEquals(AgentState.Resting, repo.state("renoa"))
        assertNull(repo.chat("renoa").streaming)
    }

    @Test
    fun stoppingAnAgentThatWantsYouOrIsAsleepChangesNothing() = runTest {
        val repo = repository()
        for (id in listOf("tifa", "mochi")) {
            val before = repo.state(id)
            repo.stop(id)
            assertEquals(before, repo.state(id))
        }
    }

    @Test
    fun aSecondMessageWhileWorkingIsAnsweredAfterTheFirstInOrder() = runTest {
        val repo = repository()
        repo.send("nagisa", "first")
        runCurrent()
        repo.send("nagisa", "second")
        settle()
        val speakers = repo.chat("nagisa").messages.map { it.speaker }
        assertEquals(listOf(Speaker.Owner, Speaker.Owner, Speaker.Agent, Speaker.Agent), speakers)
        assertEquals(AgentState.Resting, repo.state("nagisa"))
    }

    @Test
    fun aMessageSentAfterATurnEndedStartsANewTurn() = runTest {
        val repo = repository()
        repo.send("nagisa", "first")
        settle()
        repo.send("nagisa", "second")
        settle()
        assertEquals(4, repo.chat("nagisa").messages.size)
    }

    @Test
    fun everyWayIntoWorkingStartsFromTheOwnersMessage() = runTest {
        var attempts = 0
        val failsOnce = ReplyScript { _, _ -> if (attempts++ == 0) Reply.Fails else Reply.Words("ok") }
        val repo = repository(failsOnce)
        // Resting, Wants and Asleep each go to Working when the owner writes.
        for (id in listOf("sensei", "tifa", "mochi")) {
            val before = repo.state(id)
            assertTrue(before != AgentState.Working, "$id starts as $before")
        }
        repo.send("sensei", "hi")
        runCurrent()
        assertEquals(AgentState.Working, repo.state("sensei"), "from resting")
        settle()
        assertEquals(AgentState.Stuck, repo.state("sensei"), "a reply that fails leaves the agent stuck")

        repo.send("sensei", "try again")
        runCurrent()
        assertEquals(AgentState.Working, repo.state("sensei"), "from stuck")
        settle()

        repo.send("tifa", "here is my answer")
        runCurrent()
        assertEquals(AgentState.Working, repo.state("tifa"), "from wants")
        settle()

        repo.send("mochi", "wake up")
        runCurrent()
        assertEquals(AgentState.Working, repo.state("mochi"), "from asleep")
        settle()
        assertEquals(AgentState.Resting, repo.state("mochi"))
    }

    @Test
    fun theDemoScriptGetsLedgerStuckAndNobodyElse() = runTest {
        val repo = FakeAgentRepository(backgroundScope, FakeWorld.demo(), wordDelayMs = 10, newId = { ids.next() })
        repo.send("ledger", "hello")
        repo.send("nagisa", "hello")
        settle()
        assertEquals(AgentState.Stuck, repo.state("ledger"))
        assertEquals(AgentState.Resting, repo.state("nagisa"))
    }

    @Test
    fun blankWordsAreRefusedAndKeepNothing() = runTest {
        val repo = repository()
        val before = repo.chat("nagisa").messages
        assertEquals(Problem.Invalid, assertFailsWith<RepositoryException> { repo.send("nagisa", "   ") }.problem)
        assertEquals(before, repo.chat("nagisa").messages)
        assertEquals(AgentState.Resting, repo.state("nagisa"))
    }

    @Test
    fun anUnknownAgentIsNotFoundEverywhere() = runTest {
        val repo = repository()
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.send("nobody", "hi") }.problem)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.stop("nobody") }.problem)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.profile("nobody") }.problem)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.conversation("nobody") }.problem)
    }

    @Test
    fun aStreamedReplyKeepsOneIdFromTheFirstWordToTheWholeMessage() = runTest {
        val repo = repository()
        repo.send("nagisa", "hello")
        advanceTimeBy(750)
        val streamingId = repo.chat("nagisa").streaming!!.id
        settle()
        assertEquals(streamingId, repo.chat("nagisa").messages.last().id)
    }

    @Test
    fun aSeededChatGoesOnWithFreshIds() = runTest {
        val repo = repository()
        val seeded = repo.chat("renoa").let { chat -> chat.messages.map { it.id } + listOfNotNull(chat.streaming?.id) }
        repo.send("renoa", "and the other two?")
        settle()
        val ids = repo.chat("renoa").messages.map { it.id }
        assertEquals(ids.size, ids.toSet().size, "every message has its own id")
        assertTrue(ids.drop(2).none { it in seeded }, "new messages never reuse a seeded id")
    }

    // --- asking and reading ---

    private fun FakeAgentRepository.glance(id: String) = agents.value.first { it.id == id }.glance

    @Test
    fun answeringKeepsTheAnswerAsTheOwnersMessageClearsTheQuestionAndTheAgentWorks() = runTest {
        val repo = repository()
        val ask = assertNotNull(repo.glance("tifa").ask)
        val before = repo.chat("tifa").messages.size
        repo.answer("tifa", ask.choices.first())
        assertNull(repo.glance("tifa").ask)
        assertTrue(!repo.glance("tifa").unread)
        val kept = repo.chat("tifa").messages
        assertEquals(before + 1, kept.size)
        assertEquals(Speaker.Owner to ask.choices.first(), kept.last().let { it.speaker to it.text })
        runCurrent()
        assertEquals(AgentState.Working, repo.state("tifa"))
    }

    @Test
    fun anAnswerThatWasNotOfferedOrHasNoQuestionIsRefusedAndChangesNothing() = runTest {
        val repo = repository()
        val glance = repo.glance("tifa")
        val chat = repo.chat("tifa").messages
        assertEquals(Problem.Invalid, assertFailsWith<RepositoryException> { repo.answer("tifa", "Pizza") }.problem)
        assertEquals(Problem.Conflict, assertFailsWith<RepositoryException> { repo.answer("nagisa", "Yes") }.problem)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.answer("nobody", "Yes") }.problem)
        assertEquals(glance, repo.glance("tifa"))
        assertEquals(chat, repo.chat("tifa").messages)
        assertEquals(AgentState.Wants, repo.state("tifa"))
    }

    @Test
    fun aReplyIsUnreadUntilTheOwnerSeesIt() = runTest {
        val repo = repository()
        repo.send("nagisa", "hello")
        settle()
        assertTrue(repo.glance("nagisa").unread)
        assertEquals("one two three", repo.glance("nagisa").line)
        repo.markSeen("nagisa")
        assertTrue(!repo.glance("nagisa").unread)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.markSeen("nobody") }.problem)
    }

    @Test
    fun whileWorkingTheAgentSaysWhatItIsDoingAndStopsSayingItAfter() = runTest {
        val repo = repository()
        repo.send("nagisa", "hello")
        runCurrent()
        assertEquals("Checking this week's prices", repo.glance("nagisa").doing)
        settle()
        assertNull(repo.glance("nagisa").doing)
    }

    // --- creating ---

    private fun request(op: String = "op-1", name: String = "Mira", purpose: String = "Keeps the plants alive.", lit: Set<String> = setOf("calendar", "notes")) =
        CreateAgent(op, name, purpose, lit)

    @Test
    fun creatingMakesOneAgentWithTheChosenSwitchesLit() = runTest {
        val repo = repository()
        val before = repo.agents.value.size
        val made = repo.create(request())
        assertEquals(before + 1, repo.agents.value.size)
        assertEquals(setOf("calendar", "notes"), made.abilities.filter { it.lit }.map { it.id }.toSet())
        assertEquals(AgentState.Resting, repo.state(made.id))
        assertEquals(emptyList(), repo.chat(made.id).messages)
    }

    @Test
    fun theSameRequestAgainIsTheSameAgentNotASecondOne() = runTest {
        val repo = repository()
        val first = repo.create(request())
        val sizeAfterFirst = repo.agents.value.size
        val again = repo.create(request())
        assertEquals(first, again)
        assertEquals(sizeAfterFirst, repo.agents.value.size)
    }

    @Test
    fun theSameOperationWithOtherContentIsAConflictAndChangesNothing() = runTest {
        val repo = repository()
        val first = repo.create(request())
        val clash = assertFailsWith<RepositoryException> { repo.create(request(purpose = "Something else")) }
        assertEquals(Problem.Conflict, clash.problem)
        assertEquals("Keeps the plants alive.", repo.profile(first.id).purpose)
    }

    @Test
    fun aRefusedRequestLeavesNoAgentAndNoReceiptSoACorrectedRetryWorks() = runTest {
        val repo = repository()
        val before = repo.agents.value
        for (bad in listOf(request(name = "  "), request(lit = setOf("calendar", "teleport")), request(lit = setOf("wallet")), request(purpose = "a".repeat(2000)))) {
            assertEquals(Problem.Invalid, assertFailsWith<RepositoryException> { repo.create(bad) }.problem)
            assertEquals(before, repo.agents.value, "a refused create left an agent behind")
        }
        // The operation id was never used up by the refusals.
        val made = repo.create(request())
        assertEquals("Mira", made.name)
        assertEquals(before.size + 1, repo.agents.value.size)
    }

    @Test
    fun machineAbilitiesCanBeChosenWhenAnAgentIsMade() = runTest {
        val repo = repository()
        val made = repo.create(request(lit = setOf("files", "commands")))
        assertEquals(setOf("files", "commands"), made.abilities.filter { it.lit }.map { it.id }.toSet())
    }

    // --- editing ---

    @Test
    fun anEditStatesWhatItExpectsSoAStaleOneIsAConflict() = runTest {
        val repo = repository()
        val clash = assertFailsWith<RepositoryException> { repo.edit("tifa", EditProfile("Someone else", "Tif", "x", "y")) }
        assertEquals(Problem.Conflict, clash.problem)
        assertEquals("Tifa", repo.profile("tifa").name)
    }

    @Test
    fun anEditThatMatchesRenamesTheAgentEverywhere() = runTest {
        val repo = repository()
        val before = repo.profile("tifa")
        val after = repo.edit("tifa", EditProfile(before.name, "Tifa Lockhart", before.purpose, "Trains hard."))
        assertEquals("Tifa Lockhart", after.name)
        assertEquals("Tifa Lockhart", repo.agents.value.first { it.id == "tifa" }.name)
        assertEquals("Trains hard.", repo.profile("tifa").purpose)
    }

    @Test
    fun anInvalidEditChangesNothing() = runTest {
        val repo = repository()
        val before = repo.profile("tifa")
        assertEquals(Problem.Invalid, assertFailsWith<RepositoryException> { repo.edit("tifa", EditProfile(before.name, " ", before.purpose, before.purpose)) }.problem)
        assertEquals(before, repo.profile("tifa"))
    }

    @Test
    fun anEverydayAbilitySwitchesOnAndOffAfterCreation() = runTest {
        val repo = repository()
        assertTrue(repo.setAbility("tifa", "maps", true).lit)
        assertTrue(repo.profile("tifa").abilities.first { it.id == "maps" }.lit)
        assertTrue(!repo.setAbility("tifa", "maps", false).lit)
    }

    @Test
    fun aMachineAbilityCannotBeSwitchedAfterTheAgentIsMadeInEitherDirection() = runTest {
        val repo = repository()
        val machine = repo.profile("tifa").abilities.filter { it.kind == AbilityKind.Machine }
        assertEquals(setOf("phone", "files", "commands"), machine.map { it.id }.toSet())
        for (ability in machine) {
            assertEquals(Problem.Invalid, assertFailsWith<RepositoryException> { repo.setAbility("tifa", ability.id, !ability.lit) }.problem)
        }
        assertEquals(machine, repo.profile("tifa").abilities.filter { it.kind == AbilityKind.Machine })
    }

    @Test
    fun anAbilityThatIsNotBuiltYetCannotBeSwitchedAndAnUnknownOneIsNotFound() = runTest {
        val repo = repository()
        assertEquals(Problem.Invalid, assertFailsWith<RepositoryException> { repo.setAbility("tifa", "wallet", true) }.problem)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.setAbility("tifa", "teleport", true) }.problem)
    }

    @Test
    fun theSeededAgentShowsWhatItAddedItself() = runTest {
        val health = repository().profile("tifa").abilities.first { it.id == "health" }
        assertTrue(health.lit && health.addedByAgent)
    }
}

/** Lets every pending delay run. `advanceUntilIdle` would skip them: the fake's work lives in the background scope. */
@OptIn(ExperimentalCoroutinesApi::class)
private suspend fun TestScope.settle() {
    advanceTimeBy(60_000)
    runCurrent()
}
