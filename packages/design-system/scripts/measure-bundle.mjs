// Node 24+: pnpm --filter @gitust/design-system exec node scripts/measure-bundle.mjs
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { cpus, release, tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";
import { build, version as viteVersion } from "vite";

const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const levels = [1, 6, 9];
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

function attribute(source, name) {
  const pattern = new RegExp(
    `(?:^|\\s)${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s"'=<>\\x60]+))`,
    "gi",
  );
  const matches = [...source.matchAll(pattern)];
  if (matches.length > 1) throw new Error(`Duplicate HTML ${name} attribute`);
  return matches[0]?.slice(1).find((value) => value !== undefined);
}

function htmlRoots(html) {
  const scripts = [],
    preloads = [];
  for (const match of html.matchAll(/<(script|link)\b([^>]*)>/gi)) {
    const [, tag, attributes] = match;
    if (
      tag.toLowerCase() === "script" &&
      attribute(attributes, "type")?.toLowerCase() === "module"
    ) {
      const source = attribute(attributes, "src");
      if (!source)
        throw new Error("Inline HTML modules require separate import tracing");
      scripts.push(source);
    } else if (
      tag.toLowerCase() === "link" &&
      attribute(attributes, "rel")
        ?.toLowerCase()
        .split(/\s+/)
        .includes("modulepreload")
    ) {
      const href = attribute(attributes, "href");
      if (!href) throw new Error("HTML modulepreload has no href");
      preloads.push(href);
    }
  }
  const localPath = (value) => {
    const url = new URL(value, "https://bundle.invalid/");
    if (url.origin !== "https://bundle.invalid" || url.search || url.hash)
      throw new Error(
        "Cannot measure an external or qualified HTML module root",
      );
    return decodeURIComponent(url.pathname).replace(/^\//, "");
  };
  if (!scripts.length) throw new Error("No HTML module scripts were emitted");
  return { scripts: scripts.map(localPath), preloads: preloads.map(localPath) };
}

async function measure() {
  if (Number(process.versions.node.split(".")[0]) < 24)
    throw new Error("Bundle measurement requires Node 24 or newer");
  const configPath = path.join(packageRoot, "vite.config.ts");
  const { default: productionConfig } = await import(pathToFileURL(configPath));
  if (!productionConfig || typeof productionConfig !== "object")
    throw new Error("Expected the production Vite config to export an object");
  const temporary = await mkdtemp(
    path.join(tmpdir(), `gitust-bundle-${process.pid}-`),
  );
  const previousDirectory = process.cwd();
  let result;
  try {
    process.chdir(packageRoot); // Tailwind's existing config resolves relative content paths.
    const outDir = path.join(temporary, "dist");
    const chunks = new Map();
    let overview;
    await build({
      ...productionConfig,
      root: packageRoot,
      configFile: false,
      cacheDir: path.join(temporary, "cache"),
      logLevel: "silent",
      build: {
        ...productionConfig.build,
        outDir,
        emptyOutDir: true,
        sourcemap: false,
      },
      plugins: [
        ...(productionConfig.plugins ?? []),
        {
          name: "measure-emitted-static-closure",
          generateBundle(_options, bundle) {
            for (const chunk of Object.values(bundle)) {
              if (chunk.type !== "chunk") continue;
              chunks.set(chunk.fileName, {
                imports: chunk.imports,
                dynamic_imports: chunk.dynamicImports,
                entry: chunk.isEntry,
              });
              if (
                Object.keys(chunk.modules).some((id) =>
                  id
                    .replaceAll("\\", "/")
                    .endsWith("/showcase/RepositoryOverview.tsx"),
                )
              )
                overview = chunk.fileName;
            }
          },
        },
      ],
    });
    const roots = htmlRoots(
      await readFile(path.join(outDir, "index.html"), "utf8"),
    );
    const closure = (entries) => {
      const visited = new Set();
      const visit = (name) => {
        if (visited.has(name)) return;
        const chunk = chunks.get(name);
        if (!chunk) throw new Error(`Unmeasured static import: ${name}`);
        visited.add(name);
        chunk.imports.forEach(visit);
      };
      entries.forEach(visit);
      return [...visited].sort();
    };
    const emitted = new Map();
    // Read physical files only after Vite has finished every output hook.
    for (const name of chunks.keys()) {
      const bytes = await readFile(path.join(outDir, name));
      emitted.set(name, {
        file: name,
        raw_bytes: bytes.length,
        sha256: digest(bytes),
        gzip_bytes: Object.fromEntries(
          levels.map((level) => [level, gzipSync(bytes, { level }).length]),
        ),
      });
    }
    const sum = (files) => ({
      files: files.map((name) => emitted.get(name)),
      raw_bytes: files.reduce(
        (total, name) => total + emitted.get(name).raw_bytes,
        0,
      ),
      gzip_bytes: Object.fromEntries(
        levels.map((level) => [
          level,
          files.reduce(
            (total, name) => total + emitted.get(name).gzip_bytes[level],
            0,
          ),
        ]),
      ),
    });
    const initialRoots = [...roots.scripts, ...roots.preloads];
    const initial = sum(closure(initialRoots));
    result = {
      environment: {
        node: process.version,
        vite: viteVersion,
        zlib: process.versions.zlib,
        platform: process.platform,
        os_release: release(),
        arch: process.arch,
        cpu: cpus()[0]?.model,
      },
      config_sha256: digest(await readFile(configPath)),
      method:
        "Sum each physical emitted JS file compressed separately; follow all HTML module/modulepreload roots and transitive static imports. Bytes; kB=1000 bytes.",
      html_module_roots: roots.scripts,
      html_modulepreload_roots: roots.preloads,
      initial,
      optional_first_repository_overview: overview
        ? sum(closure([...initialRoots, overview]))
        : null,
      graph: Object.fromEntries(
        [...chunks].sort(([a], [b]) => a.localeCompare(b)),
      ),
      initial_target_bytes: 50000,
      initial_target_met: initial.gzip_bytes[9] < 50000,
      startup_dynamic_cohorts:
        "Not measured here. Confirm automatic dynamic imports and real cold-load requests separately in the browser; optional chunks are not proof of startup latency.",
    };
  } finally {
    try {
      process.chdir(previousDirectory);
    } finally {
      await rm(temporary, { recursive: true, force: true });
    }
  }
  return { ...result, temporary_build_removed: true };
}

try {
  console.log(JSON.stringify(await measure(), null, 2));
} catch (error) {
  console.error(
    `Bundle measurement failed: ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
}
