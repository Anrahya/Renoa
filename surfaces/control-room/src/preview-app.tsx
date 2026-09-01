import { usePreviewControlRoom } from "./use-preview-control-room";
import { Workspace } from "./workspace";

export default function PreviewApp() {
  const control = usePreviewControlRoom();
  return <Workspace control={control} preview />;
}
