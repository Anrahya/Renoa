import { useEffect, useState } from "react";
import { ArrowRight, ArrowUp, ArrowUpRight, Pause, Play, Plus } from "@phosphor-icons/react";
import { SystemForm } from "./system-form";
import { homepageHostEntry } from "./host-entry";
import "./styles/landing.css";

const parts = [
  {
    name: "Models",
    title: "Choose a model for each agent.",
    description: "Assign the model that fits the work. Models and providers are configurable parts of Renoa.",
  },
  {
    name: "Tools",
    title: "Share tools through the Host.",
    description: "Connect tools to your Host and make them available to the agents that need them.",
  },
  {
    name: "Agents",
    title: "Define agents for specific work.",
    description: "Combine instructions, a model, and tools in each agent. The Host keeps their records and schedules together.",
  },
] as const;

export function LandingPage() {
  const hostEntry = homepageHostEntry(import.meta.env.DEV);
  const [selected, setSelected] = useState<number | null>(null);
  const [paused, setPaused] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const part = selected === null ? null : parts[selected];

  useEffect(() => {
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    function update() { setReducedMotion(preference.matches); }
    update();
    preference.addEventListener("change", update);
    return () => preference.removeEventListener("change", update);
  }, []);

  function selectPart(index: number, toggle = true) {
    setSelected(toggle && selected === index ? null : index);
    // Keep the explanation in sight on shorter screens after the selection
    // commits, without moving keyboard focus away from the chosen part.
    window.requestAnimationFrame(() => {
      document.getElementById("system-explanation")?.scrollIntoView({
        block: "nearest", behavior: reducedMotion ? "instant" : "smooth",
      });
    });
  }

  function explore() {
    selectPart(0, false);
    document.getElementById("system-models")?.focus({ preventScroll: true });
  }

  return <div className="renoa-home">
    <a className="renoa-skip" href="#home-main">Skip to content</a>
    <header className="renoa-home-header">
      <a href="/" className="renoa-wordmark" aria-label="Renoa home">renoa<span>.</span></a>
      <nav aria-label="Main navigation">
        <a className="renoa-nav-idea" href="#the-idea">The idea</a>
        <a className="renoa-nav-source" href="https://github.com/Anrahya/Renoa">Source <ArrowUpRight size={16} aria-hidden="true" /></a>
        <a className="renoa-open" href={hostEntry.href}>{hostEntry.label} <ArrowUpRight size={17} aria-hidden="true" /></a>
      </nav>
    </header>

    <main id="home-main">
      <section className="renoa-hero" aria-labelledby="home-heading">
        <div className="renoa-introduction">
          <h1 id="home-heading">A modular<br />AI <em>system.</em></h1>
          <p>Create agents from models, instructions, and tools.<br />Manage them through one shared Host.</p>
          <button className="renoa-text-action" onClick={explore}>Explore the system <ArrowRight size={21} aria-hidden="true" /></button>
        </div>

        <figure className="renoa-system" aria-label="Explore how models, tools, and agents connect through a Renoa Host">
          <SystemForm selected={selected} paused={paused || reducedMotion} />
          <a className="renoa-form-host" href={hostEntry.href} aria-label={hostEntry.label}>r<ArrowUpRight size={11} aria-hidden="true" /></a>
          <div className="renoa-form-parts" role="group" aria-label="Parts of the system">
            {parts.map((item, index) => <button
              key={item.name}
              id={`system-${item.name.toLowerCase()}`}
              className={`renoa-part renoa-part-${index}`}
              aria-pressed={selected === index}
              aria-controls="system-explanation"
              onClick={() => selectPart(index)}
            ><span className="renoa-part-mark" aria-hidden="true"><Plus size={14} weight="bold" /></span>{item.name}</button>)}
          </div>
          <figcaption className="renoa-form-caption">Models, tools, and agents connected through one Host.</figcaption>
          {!reducedMotion && <button className="renoa-motion" onClick={() => setPaused(!paused)} aria-label={paused ? "Play visual motion" : "Pause visual motion"}>
            {paused ? <Play size={16} aria-hidden="true" /> : <Pause size={16} aria-hidden="true" />}
          </button>}
        </figure>
      </section>

      <section className="renoa-explanation" id="system-explanation" aria-live="polite" aria-atomic="true">
        <div className="renoa-explanation-heading"><span className="renoa-small-orbit" aria-hidden="true" />
          <h2>{part?.title ?? "Configure the parts of your AI system."}</h2>
        </div>
        <p>{part?.description ?? "Renoa combines agents, models, instructions, and tools under one shared Host."}</p>
      </section>

      <section className="renoa-idea" id="the-idea" aria-labelledby="idea-heading">
        <a href="#home-main" className="renoa-idea-mark" aria-label="Return to the system"><ArrowUp size={25} aria-hidden="true" /></a>
        <div>
          <h2 id="idea-heading">Configure each agent.<br />Manage them from one Host.</h2>
          <div className="renoa-idea-body">
            <p>Choose an agent’s instructions, model, and tools for the work it performs. Update that configuration as requirements change.</p>
            <p>The Host stores agents, records, schedules, and shared connections. Surfaces such as Slack, GitHub, and the browser connect to it.</p>
          </div>
          <a href={hostEntry.href} className="renoa-text-action">{hostEntry.label} <ArrowUpRight size={21} aria-hidden="true" /></a>
        </div>
      </section>
    </main>

    <footer className="renoa-home-footer">
      <a href="/" className="renoa-wordmark" aria-label="Renoa home">renoa<span>.</span></a>
      <p>A modular AI system managed through one Host.</p>
      <a href="https://github.com/Anrahya/Renoa">Built in the open <ArrowUpRight size={16} aria-hidden="true" /></a>
    </footer>
  </div>;
}
