/** Renoa reference components. Tile and Rosette are drawn by the bundle (window.Renoa);
 *  the rest are CSS components: markup plus rn- classes and data attributes from bundle.css. */

export type PartId = 'model' | 'loop' | 'context' | 'tools' | 'skills' | 'profile';

/** Tile: one hex tile. Renoa.Tile(host, props) appends an <svg class="rn-tile">. */
export interface TileProps {
  /** core is fixed ink; part wears its pigment; plugin wears the Tools edge; proposed, surface, control and retiring are neutral. */
  kind?: 'core' | 'part' | 'plugin' | 'proposed' | 'surface' | 'control' | 'retiring';
  /** Required when kind is part. */
  part?: PartId;
  /** Name written inside the tile, 1 or 2 short words. */
  label?: string;
  /** Mono line under the name: a version, 'plugin', 'never changes'. */
  meta?: string;
  /** Circumradius in px. Default 60. */
  size?: number;
  /** Body height under the face at full size. Default 4 (tile-depth); 0 draws a flat tile. */
  depth?: number;
  ariaLabel?: string;
}
export declare function Tile(host: Element | string, props: TileProps): SVGSVGElement;

/** Rosette: an agent drawn as its tiles. Renoa.Rosette(host, props) appends an <svg class="rn-rosette">. */
export interface RosetteProps {
  /** Rendered width in px. Default 160. Below 80px grout widens so tiles stay distinct. */
  size?: number;
  /** Parts that are set. Default all six. Unset seats render as empty sockets. */
  parts?: PartId[];
  /** Plugin count, or names (names show only with labels). */
  plugins?: number | string[];
  /** Plugins waiting for the owner to approve. Drawn dashed with a needs-you dot. */
  proposed?: number;
  /** Write names inside tiles. Use at 280px wide and up. */
  labels?: boolean;
  /** Per-part version strings shown under names when labels is true. */
  versions?: Partial<Record<PartId, string>>;
  /** Agent name for the accessible label. */
  name?: string;
  /** Body height under each face at full size. Default 4 (tile-depth); 0 draws flat tiles. */
  depth?: number;
}
export declare function Rosette(host: Element | string, props: RosetteProps): SVGSVGElement;

/** Flat-top hex path with rounded corners, for drawing your own boards. */
export declare function hexPath(cx: number, cy: number, r: number, corner?: number): string;
/** Axial hex coordinates to x, y for circumradius R (flat-top). */
export declare function axial(q: number, r: number, R: number): [number, number];
export declare const PARTS: { id: PartId; name: string; q: number; r: number }[];
export declare const GROWTH: [number, number][];

/** PartChip: <span class="rn-part" data-part="model">Model <span class="rn-ver">v3</span></span>. data-kind="plugin" or "core" for those. */
export interface PartChipProps { part?: PartId; kind?: 'plugin' | 'core'; version?: string; }

/** StateBadge: <span class="rn-state" data-state="needs-you">Needs you</span>. Always keep the word. */
export interface StateBadgeProps { state: 'needs-you' | 'running' | 'failed' | 'idle' | 'done'; }

/** Button: <button class="rn-btn" data-variant data-size>. Primary is the default. */
export interface ButtonProps { variant?: 'primary' | 'secondary' | 'ghost' | 'destructive'; size?: 'md' | 'sm'; disabled?: boolean; }

/** Tabs: <div class="rn-tabs" role="tablist"> of <button class="rn-tab" role="tab" aria-selected>. */
export interface TabsProps { selected: string; }

/** Field: <div class="rn-field"> label, .rn-input (input, select, textarea), .rn-hint, .rn-error. data-invalid on error. */
export interface FieldProps { label: string; hint?: string; error?: string; mono?: boolean; }

/** Switch: <label class="rn-switch"><input type="checkbox" role="switch"> Label</label>. */
export interface SwitchProps { checked?: boolean; disabled?: boolean; }

/** RecordRow: <ul class="rn-rows"> of <li><a class="rn-row"> with .rn-row-main (.rn-row-title, .rn-row-meta) and .rn-row-end. data-attention washes a row that needs the owner. */
export interface RecordRowProps { title: string; meta?: string[]; attention?: boolean; }

/** Kicker: <div class="rn-kicker"><span class="rn-fig"><span>A</span></span>The core<span class="rn-rule"></span></div>. */
export interface KickerProps { letter?: string; label: string; }

/** Plate: <figure class="rn-plate"> with .rn-plate-stage (add rn-board for the lattice) and .rn-plate-cap. */
export interface PlateProps { figure: string; title: string; text?: string; }

/** Note: <div class="rn-note"> with .rn-note-head (eyebrow, optional .rn-tag), .rn-note-text and optional .rn-note-rows.
 *  Join it to its tile with an SVG <path class="rn-leader"> ending in <circle class="rn-leader-dot">.
 *  In running text, <span class="rn-cue" tabindex="0"> marks words that point at the board; aria-current marks the ones being shown. */
export interface NoteProps { eyebrow: string; text?: string; rows?: [string, string][]; built?: boolean; }
