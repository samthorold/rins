// Tiny static file server for the UI.
//
// ES module imports break under file:// in Chrome, so dev needs an http
// origin. Run `npm run serve` from ui/ (or `node serve.js`) and visit
// http://localhost:8000.
//
// Serves files from the ui/ directory and from the parent (so events.ndjson
// can be fetched via /events.ndjson without copying it).

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { join, resolve, extname, normalize } from "node:path";

const here = resolve(new URL(".", import.meta.url).pathname);
const repoRoot = resolve(here, "..");
const port = Number(process.env.PORT || 8000);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".ndjson": "application/x-ndjson; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
};

async function resolvePath(urlPath) {
  // Strip query, decode, normalise.
  const clean = normalize(decodeURIComponent(urlPath.split("?")[0]));
  const candidates = [
    join(here, clean === "/" ? "index.html" : clean),
    join(repoRoot, clean), // allow /events.ndjson at repo root
  ];
  for (const p of candidates) {
    if (!p.startsWith(here) && !p.startsWith(repoRoot)) continue;
    try {
      const st = await stat(p);
      if (st.isDirectory()) {
        const idx = join(p, "index.html");
        try {
          await stat(idx);
          return idx;
        } catch {}
        continue;
      }
      return p;
    } catch {}
  }
  return null;
}

const server = createServer(async (req, res) => {
  const file = await resolvePath(req.url || "/");
  if (!file) {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found");
    return;
  }
  try {
    const data = await readFile(file);
    res.writeHead(200, {
      "content-type": MIME[extname(file)] || "application/octet-stream",
      "cache-control": "no-cache",
    });
    res.end(data);
  } catch (err) {
    res.writeHead(500, { "content-type": "text/plain" });
    res.end(String(err));
  }
});

server.listen(port, () => {
  console.log(`rins UI on http://localhost:${port}`);
});
