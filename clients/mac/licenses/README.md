# Third-party material in the Mac app

| What | Where in the app | Source | Licence |
|---|---|---|---|
| The black-and-white palette, light and dark, and the chat layout it is drawn in: agents down the side, a title pill, the owner's bubbles on the right, a rounded message box | `src/theme.rs`, `src/chats.rs`, `src/app.rs` | leepokai/Codync `apps/desktop/src/renderer/styles/theme.css` and `views/` at commit `df80259e2dae59543df648d1a20aa3e3b350e758`, as values and layout rebuilt in GPUI, without its names, logos, icons or avatars | Apache-2.0 with Codync's NOTICE, `../android/licenses/Apache-2.0-Codync.txt` |

GPUI, gpui-kit and the other crates the app links are dependencies, not adapted
code; `Cargo.lock` names each version.
