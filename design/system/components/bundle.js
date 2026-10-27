/* @ds-bundle: {"format":4,"namespace":"Renoa","components":[{"name":"Tile"},{"name":"Rosette"},{"name":"PartChip"},{"name":"StateBadge"},{"name":"Button"},{"name":"Tabs"},{"name":"Field"},{"name":"Switch"},{"name":"RecordRow"},{"name":"Kicker"},{"name":"Plate"},{"name":"Note"}]} */
(function () {
  var NS = 'http://www.w3.org/2000/svg';
  var S3 = Math.sqrt(3);

  /* The six replaceable parts, clockwise from the top, and their seat around the core. */
  var PARTS = [
    { id: 'model', name: 'Model', q: 0, r: -1 },
    { id: 'loop', name: 'Workflow', q: 1, r: -1 },
    { id: 'context', name: 'Context', q: 1, r: 0 },
    { id: 'tools', name: 'Tools', q: 0, r: 1 },
    { id: 'skills', name: 'Skills', q: -1, r: 1 },
    { id: 'profile', name: 'Profile', q: -1, r: 0 }
  ];
  /* Where plugins land, in order. They add to Tools, so they gather under the Tools tile. */
  var GROWTH = [[0, 2], [1, 1], [-1, 2], [1, 2], [-1, 3], [0, 3], [2, 1], [-2, 3], [2, 2], [-2, 4], [1, 3], [-1, 4]];

  function axial(q, r, R) { return [R * 1.5 * q, R * S3 * (r + q / 2)]; }

  /* Flat-top hexagon with rounded corners. */
  function hexPath(cx, cy, r, corner) {
    var c = Math.min(corner || 0, r * 0.3), p = [], d = '', i;
    for (i = 0; i < 6; i++) p.push([cx + r * Math.cos(Math.PI / 3 * i), cy + r * Math.sin(Math.PI / 3 * i)]);
    for (i = 0; i < 6; i++) {
      var a = p[(i + 5) % 6], b = p[i], n = p[(i + 1) % 6], la = Math.hypot(a[0] - b[0], a[1] - b[1]), ln = Math.hypot(n[0] - b[0], n[1] - b[1]);
      var s = [b[0] + (a[0] - b[0]) * c / la, b[1] + (a[1] - b[1]) * c / la], e = [b[0] + (n[0] - b[0]) * c / ln, b[1] + (n[1] - b[1]) * c / ln];
      d += (i ? 'L' : 'M') + s[0].toFixed(2) + ' ' + s[1].toFixed(2) + 'Q' + b[0].toFixed(2) + ' ' + b[1].toFixed(2) + ' ' + e[0].toFixed(2) + ' ' + e[1].toFixed(2);
    }
    return d + 'Z';
  }

  function el(tag, attrs, parent) {
    var n = document.createElementNS(NS, tag);
    for (var k in attrs) if (attrs[k] !== undefined && attrs[k] !== null) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }

  /* One tile group at (x, y). kind: core | part | plugin | proposed | surface | control | retiring.
     A tile is a face on a short body (tile-depth); a proposed tile has no body yet. */
  function tileGroup(parent, o) {
    var R = o.R || 60, grout = o.grout == null ? 5 : o.grout, depth = o.depth == null ? 4 : o.depth;
    var g = el('g', { 'data-kind': o.kind || 'plugin', 'data-part': o.part, transform: 'translate(' + (o.x || 0) + ' ' + (o.y || 0) + ')' }, parent);
    var face = hexPath(0, 0, R - grout / 2, o.corner == null ? 3 : o.corner);
    if (depth > 0 && o.kind !== 'proposed') el('path', { 'class': 'rn-side', d: face, transform: 'translate(0 ' + depth + ')' }, g);
    el('path', { 'class': 'rn-hex', d: face }, g);
    if (o.label) {
      var two = !!o.meta;
      var t = el('text', { 'class': 'rn-tname', x: 0, y: two ? -7 : 0 }, g); t.textContent = o.label;
      if (two) { var m = el('text', { 'class': 'rn-tmeta', x: 0, y: 11 }, g); m.textContent = o.meta; }
    }
    if (o.kind === 'proposed') el('circle', { 'class': 'rn-ask', cx: R * 0.48, cy: -R * 0.5, r: 4 }, g);
    return g;
  }

  function mount(host, svg) {
    if (typeof host === 'string') host = document.querySelector(host);
    if (host) host.appendChild(svg);
    return svg;
  }

  /* Tile(host, {kind, part, label, meta, size}) draws a single tile. size = circumradius in px. */
  function Tile(host, o) {
    o = o || {};
    var R = 60, size = o.size || 60, w = 2 * R, h = S3 * R, depth = o.depth == null ? 4 : o.depth;
    var svg = el('svg', { 'class': 'rn-tile', viewBox: (-w / 2 - 2) + ' ' + (-h / 2 - 2) + ' ' + (w + 4) + ' ' + (h + 4 + depth), width: (size * 2 + 4 * size / 60).toFixed(1), role: 'img', 'aria-label': o.ariaLabel || [o.label, o.meta].filter(Boolean).join(', ') || (o.kind || 'tile') });
    tileGroup(svg, { kind: o.kind || (o.part ? 'part' : 'plugin'), part: o.part, label: o.label, meta: o.meta, R: R, depth: depth });
    return mount(host, svg);
  }

  /* Rosette(host, opts) draws an agent as tiles: core, six parts, then what it has added.
     opts.size      rendered width in px (default 160)
     opts.parts     part ids that are set (default all six); unset seats show as empty sockets
     opts.plugins   number of plugins, or an array of plugin names (labels at 200px and up)
     opts.proposed  number of plugins waiting for approval
     opts.labels    true to write part names inside tiles (use at 280px and up)
     opts.versions  {model: 'v3', ...} shown under part names when labels is true
     opts.depth     body height under each face at full size (default 4, the tile-depth token; 0 is flat) */
  function Rosette(host, o) {
    o = o || {};
    var R = 60, set = o.parts || PARTS.map(function (p) { return p.id; });
    var plugins = Array.isArray(o.plugins) ? o.plugins : new Array(o.plugins || 0).join('.').split('.').slice(0, o.plugins || 0).map(function () { return ''; });
    var proposed = o.proposed || 0, cells = [];
    cells.push({ q: 0, r: 0, kind: 'core', label: o.labels ? 'Core' : null, meta: o.labels ? 'never changes' : null });
    PARTS.forEach(function (p) {
      var on = set.indexOf(p.id) >= 0;
      cells.push({ q: p.q, r: p.r, kind: on ? 'part' : 'socket', part: p.id, label: o.labels && on ? p.name : null, meta: o.labels && on && o.versions ? o.versions[p.id] : null });
    });
    plugins.forEach(function (nm, i) { var gpos = GROWTH[i]; cells.push({ q: gpos[0], r: gpos[1], kind: 'plugin', label: o.labels && nm ? nm : null, meta: o.labels && nm ? 'plugin' : null }); });
    for (var j = 0; j < proposed; j++) { var gp = GROWTH[plugins.length + j]; if (gp) cells.push({ q: gp[0], r: gp[1], kind: 'proposed', label: o.labels ? 'Approve?' : null }); }
    var x0 = 1e9, x1 = -1e9, y0 = 1e9, y1 = -1e9;
    cells.forEach(function (c) { var p = axial(c.q, c.r, R); c.x = p[0]; c.y = p[1]; x0 = Math.min(x0, p[0] - R); x1 = Math.max(x1, p[0] + R); y0 = Math.min(y0, p[1] - R * S3 / 2); y1 = Math.max(y1, p[1] + R * S3 / 2); });
    var depth = o.depth == null ? 4 : o.depth, pad = 3, vw = x1 - x0 + pad * 2, vh = y1 - y0 + depth + pad * 2, size = o.size || 160;
    var svg = el('svg', { 'class': 'rn-rosette', viewBox: [x0 - pad, y0 - pad, vw, vh].map(function (v) { return v.toFixed(1); }).join(' '), width: size, height: (size * vh / vw).toFixed(1), role: 'img',
      'aria-label': (o.name ? o.name + ': ' : '') + set.length + ' of 6 parts set' + (plugins.length ? ', ' + plugins.length + ' plugin' + (plugins.length > 1 ? 's' : '') : '') + (proposed ? ', ' + proposed + ' waiting for approval' : '') });
    cells.forEach(function (c) {
      if (c.kind === 'socket') { el('path', { 'class': 'rn-socket', d: hexPath(c.x, c.y, R - 2.5, 3) }, svg); return; }
      tileGroup(svg, { kind: c.kind, part: c.part, label: c.label, meta: c.meta, x: c.x, y: c.y, R: R, grout: size < 80 ? 9 : 5, depth: depth });
    });
    return mount(host, svg);
  }

  window.Renoa = { version: '1', PARTS: PARTS, GROWTH: GROWTH, axial: axial, hexPath: hexPath, Tile: Tile, Rosette: Rosette };
})();
