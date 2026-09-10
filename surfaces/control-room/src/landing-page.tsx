import { useEffect, useState } from "react";
import { ArrowRight, ArrowUp, ArrowUpRight, Pause, Play, Plus } from "@phosphor-icons/react";
import { SystemForm } from "./system-form";
import "./styles/landing.css";

const parts = [
  {
    name: "Models",
    title: "Change the mind. Keep the system.",
    description: "Choose the model that suits the work. Providers are replaceable parts of Renoa, so your system can evolve with them.",
  },
  {
    name: "Tools",
    title: "Connect once. Build on it.",
    description: "Connections belong to your Host. Give agents the tools they need from a shared collection, without setting up the same account again on every surface.",
  },
  {
    name: "Agents",
    title: "Different purposes. Common ground.",
    description: "Combine instructions, a model, and tools for a particular job. Each agent has its own purpose, with its records and schedules held by the Host.",
  },
] as const;

export function LandingPage() {
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
        <a className="renoa-open" href="/?host">Open your Host <ArrowUpRight size={17} aria-hidden="true" /></a>
      </nav>
    </header>

    <main id="home-main">
      <section className="renoa-hero" aria-labelledby="home-heading">
        <div className="renoa-introduction">
          <h1 id="home-heading">A system<br />of your <em>own.</em></h1>
          <p>Your models. Your tools. Your agents.<br />Connected through one personal Host.</p>
          <button className="renoa-text-action" onClick={explore}>Explore the system <ArrowRight size={21} aria-hidden="true" /></button>
        </div>

        <figure className="renoa-system" aria-label="Explore how models, tools, and agents connect through a Renoa Host">
          <SystemForm selected={selected} paused={paused || reducedMotion} />
          <a className="renoa-form-host" href="/?host" aria-label="Open your Host">r<ArrowUpRight size={11} aria-hidden="true" /></a>
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
          <figcaption className="renoa-form-caption">Separate parts. Shared possibilities.</figcaption>
          {!reducedMotion && <button className="renoa-motion" onClick={() => setPaused(!paused)} aria-label={paused ? "Play visual motion" : "Pause visual motion"}>
            {paused ? <Play size={16} aria-hidden="true" /> : <Pause size={16} aria-hidden="true" />}
          </button>}
        </figure>
      </section>

      <section className="renoa-explanation" id="system-explanation" aria-live="polite" aria-atomic="true">
        <div className="renoa-explanation-heading"><span className="renoa-small-orbit" aria-hidden="true" />
          <h2>{part?.title ?? "Many parts. A whole that’s yours."}</h2>
        </div>
        <p>{part?.description ?? "Renoa is a modular AI system. Bring the pieces together, give them a purpose, and keep shaping what your system can do."}</p>
      </section>

      <section className="renoa-idea" id="the-idea" aria-labelledby="idea-heading">
        <a href="#home-main" className="renoa-idea-mark" aria-label="Return to the system"><ArrowUp size={25} aria-hidden="true" /></a>
        <div>
          <h2 id="idea-heading">The pieces will change.<br />Your system stays yours.</h2>
          <div className="renoa-idea-body">
            <p>A new model. A useful connection. An agent with a different job. Renoa is built to make room for what comes next.</p>
            <p>Your Host holds the continuity: the agents, their records, and the connections they use. Slack, GitHub, and the browser are ways into that system.</p>
          </div>
          <a href="/?host" className="renoa-text-action">Make yourself at home <ArrowUpRight size={21} aria-hidden="true" /></a>
        </div>
      </section>
    </main>

    <footer className="renoa-home-footer">
      <a href="/" className="renoa-wordmark" aria-label="Renoa home">renoa<span>.</span></a>
      <p>A personal system. Room to evolve.</p>
      <a href="https://github.com/Anrahya/Renoa">Built in the open <ArrowUpRight size={16} aria-hidden="true" /></a>
    </footer>
  </div>;
}
