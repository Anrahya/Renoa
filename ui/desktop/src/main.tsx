import ReactDOM from 'react-dom/client';
import { RenoaDesktop } from './App';
import { TauriAcpTransport } from './acp/tauriTransport';
import './styles/theme.css';
import './styles/base.css';
import './styles/launch.css';
import './styles/app.css';

async function createTransport() {
  if (import.meta.env.DEV && new URLSearchParams(window.location.search).has('fixture')) {
    const { FixtureAcpTransport } = await import('./test/fixtureTransport');
    return new FixtureAcpTransport();
  }

  return new TauriAcpTransport('renoa-main');
}

void createTransport().then((transport) => {
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <RenoaDesktop transport={transport} />,
  );
});
