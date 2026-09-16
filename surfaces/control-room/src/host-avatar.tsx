import { GithubLogo } from "@phosphor-icons/react";
import { portraitForAgent } from "./host-identity";
import "./styles/host-avatar.css";

export function AgentAvatar({ agentId, name, github = false }: { agentId: string; name: string; github?: boolean }) {
  const portrait = portraitForAgent(agentId, name);
  return <span className="host-avatar host-avatar-portrait" aria-hidden="true">
    <img src={portrait} alt="" width="96" height="96" decoding="async" loading="lazy" />
    {github && <span className="host-avatar-surface"><GithubLogo weight="fill" size={15} /></span>}
  </span>;
}
