# Product
<!-- impeccable:product-schema 1 -->

## Platform
Web. Renoa's public front page and the authenticated browser surface of one personal Host.

## Users
Initially its owner: a developer building and using a personal AI system across a computer and phone. Future users may run their Host on a server or a spare computer.

## Product Purpose
Renoa brings models, tools, and purpose-built agents into one modular, personal system. Its Host owns durable records and shared connections; chat and development applications are surfaces into that system.

## Capabilities and Constraints
The current system runs agents, scheduled work, and GitHub reviews, with shared connections and remembered browser authentication. Slack, Telegram, GitHub, and the browser are implemented surfaces; the Slack adapter and Host attachment are documented in `../../crates/renoa-slack/README.md` and `../../deploy/renoa-slack-host.service`. Providers, tools, instructions, and runtime components have separate responsibilities.

The authenticated Host browser surface presents an overview, direct agent pages, a shared library, durable session and review records, scheduled work, and repository review policy. The live Host can pause or resume schedules and edit repository review triggers with revision-aware saves. Saved preview data is dated and read-only. Model and tool recipe editing and cross-agent collaboration remain future API work.

The public homepage is a product introduction and conceptual visualization. It contains no private work data, live agent state, execution traces, or invented telemetry. Opening the Host leads to authenticated management. This design scope covers both the public homepage and the private Host as separate surfaces in one visual system.

## Brand Commitments
Personal ownership, modularity, and room to evolve. The name is Renoa. Character-inspired agent names do not imply a character-themed interface.

## Evidence on Hand
The owner's direct product, homepage, and Host briefs; the working authenticated Host and browser implementation. Previous generated mockups are historical, not the visual source for either current surface.

## Product Principles
- Make the system understandable before exposing its controls.
- Keep shared resources and durable identity with the Host, independent of a surface.
- Prefer a continuous, legible composition over a grid of cards.
- Let a visual invite exploration without pretending to show live work or internal thoughts.
- Clearly distinguish stored records, actionable controls, and future ambitions.
