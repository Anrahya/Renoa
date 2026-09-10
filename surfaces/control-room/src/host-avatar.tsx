import { GithubLogo } from "@phosphor-icons/react";
import { genericPortrait } from "./host-identity";
import "./styles/host-avatar.css";

// Personal display artwork only. Names never determine tools, ownership, or role.
const portraits = new Map([
  ["arcee", "/assets/identities/arcee-prime.webp"],
  ["rc", "/assets/identities/arcee-prime.webp"],
  ["soundwave", "/assets/identities/soundwave-prime.webp"],
]);

export function AgentAvatar({ agentId, name, github = false }: { agentId: string; name: string; github?: boolean }) {
  const portrait = portraits.get(name.trim().toLocaleLowerCase()) ?? genericPortrait(agentId);
  return <span className="host-avatar host-avatar-portrait" aria-hidden="true">
    <img src={portrait} alt="" width="96" height="96" decoding="async" loading="lazy" />
    {github && <span className="host-avatar-surface"><GithubLogo weight="fill" size={15} /></span>}
  </span>;
}
