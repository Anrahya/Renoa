export async function pickWorkspace(): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    directory: true,
    multiple: false,
    title: 'Choose a workspace',
  });
  return typeof selected === 'string' ? selected : null;
}
