// Stage claude-code + codex + gh + glab into `sidecar/dist/vendor/`
// for Tauri to ship as bundle resources. Windows host only.
//
// Mirrors stage-vendor.ts (macOS) deliberately as a separate file rather than
// adding `if (windows)` branches into the macOS-only file, so the macOS code
// path stays byte-for-byte identical to upstream.
//
// Source layout:
//   - claude.exe   ← node_modules/@anthropic-ai/claude-code-win32-<arch>/claude.exe
//   - codex.exe + Windows-only helpers + path/rg.exe ←
//       node_modules/@openai/codex-win32-<arch>/vendor/<triple>/{codex,path}/
//   - gh.exe       ← github.com/cli/cli/releases/download/v$VER/gh_${VER}_windows_${arch}.zip
//   - glab.exe     ← gitlab.com/gitlab-org/cli/-/releases/v$VER/downloads/glab_${VER}_windows_${arch}.zip
//
// Windows-specific notes vs macOS:
//   - No code signing (Authenticode is optional; left for a later Phase).
//   - Codex Windows package ships TWO extra binaries next to codex.exe:
//       codex-command-runner.exe        (PTY helper)
//       codex-windows-sandbox-setup.exe (Windows Sandbox integration)
//     Both must be copied or codex.exe may fail at runtime when it tries to
//     spawn them. We copy the entire `codex/` directory rather than just the
//     main binary.
//   - glab has NO Windows arm64 build upstream as of v1.93.0; we only support
//     x86_64 for now and fail loudly if someone tries arm64.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	cpSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	rmSync,
	statSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPTS_DIR = dirname(fileURLToPath(import.meta.url));
const SIDECAR_ROOT = join(SCRIPTS_DIR, "..");
const NODE_MODULES = join(SIDECAR_ROOT, "node_modules");
const DIST_VENDOR = join(SIDECAR_ROOT, "dist", "vendor");
const BUNDLE_CACHE = join(SIDECAR_ROOT, ".bundle-cache");

// Bumping any version: update SHA256 below + wipe sidecar/.bundle-cache.
//   gh:          github.com/cli/cli/releases/download/v$VER/gh_${VER}_checksums.txt
//   glab:        gitlab.com/gitlab-org/cli/-/releases/v$VER/downloads/checksums.txt
//   codex:       npm pack @openai/codex@<version>-win32-<arch> → sha256
//   claude-code: npm tarball at registry.npmjs.org/@anthropic-ai/claude-code-win32-<arch>/-/...tgz

const GH_VERSION = "2.91.0";
const GH_SHA256 = {
	amd64: "ced3e6f4bb5a9865056b594b7ad0cf42137dc92c494346f1ca705b5dbf14c88e",
	arm64: "ae0333d2f9b13fc28f785ca7379514f9a1cea382cd4726abb6e6f4d2a874dd15",
} as const;

const GLAB_VERSION = "1.93.0";
const GLAB_SHA256: Readonly<Record<"amd64", string>> = {
	amd64: "e07ea21f9a3df8eac5e1c16136c186154769504355a44195b47c44e410a39097",
	// arm64: upstream has no Windows arm64 build for glab as of v1.93.0.
};

// ---------------------------------------------------------------------------
// Target detection
// ---------------------------------------------------------------------------

type WinArch = "x64" | "arm64";

interface TargetInfo {
	arch: WinArch;
	/** `@anthropic-ai/claude-code-win32-<arch>` */
	claudeCodePkg: string;
	/** `@openai/codex-win32-<arch>` */
	codexPkg: string;
	/** Target triple inside the codex platform package. */
	codexTriple: string;
	/** `gh` release naming: `amd64` / `arm64`. */
	ghArch: "amd64" | "arm64";
	/** `glab` release naming: only `amd64` available upstream. */
	glabArch: "amd64";
}

function infoForArch(arch: WinArch): TargetInfo {
	if (arch === "arm64") {
		// We can stage claude-code/codex for arm64 (npm sub-packages exist), but
		// glab has no Windows arm64 build. Fail loudly so the user knows.
		throw new Error(
			`[stage-vendor-windows] Windows arm64 is not yet supported: ` +
				`upstream glab v${GLAB_VERSION} has no Windows arm64 release. ` +
				`Open the issue or pin a Windows arm64 glab fork before staging.`,
		);
	}
	return {
		arch,
		claudeCodePkg: "@anthropic-ai/claude-code-win32-x64",
		codexPkg: "@openai/codex-win32-x64",
		codexTriple: "x86_64-pc-windows-msvc",
		ghArch: "amd64",
		glabArch: "amd64",
	};
}

function detectTarget(): TargetInfo {
	if (process.platform !== "win32") {
		throw new Error(
			`[stage-vendor-windows] expected platform=win32, got ${process.platform}`,
		);
	}
	// Respect TAURI_TARGET_TRIPLE for cross-arch staging in CI.
	const triple =
		process.env.TAURI_TARGET_TRIPLE?.trim() ||
		process.env.TAURI_ENV_TARGET_TRIPLE?.trim() ||
		process.env.CARGO_BUILD_TARGET?.trim();
	if (triple) {
		if (triple === "x86_64-pc-windows-msvc") return infoForArch("x64");
		if (triple === "aarch64-pc-windows-msvc") return infoForArch("arm64");
		throw new Error(
			`[stage-vendor-windows] unsupported TAURI_TARGET_TRIPLE: ${triple}`,
		);
	}
	const arch = process.arch;
	if (arch === "x64") return infoForArch("x64");
	if (arch === "arm64") return infoForArch("arm64");
	throw new Error(`[stage-vendor-windows] unsupported Windows host arch: ${arch}`);
}

// ---------------------------------------------------------------------------
// Copy + download + hash helpers (intentionally not shared with stage-vendor.ts
// to keep that file's import surface unchanged for macOS).
// ---------------------------------------------------------------------------

function ensureExists(path: string, label: string): void {
	if (!existsSync(path)) {
		throw new Error(
			`[stage-vendor-windows] expected ${label} at ${path} — run \`bun install\` in sidecar/ first`,
		);
	}
}

function copyFile(src: string, dest: string): void {
	mkdirSync(dirname(dest), { recursive: true });
	cpSync(src, dest);
}

function humanSize(path: string): string {
	if (!existsSync(path)) return "(missing)";
	let bytes = 0;
	const walk = (p: string): void => {
		const s = statSync(p);
		if (s.isDirectory()) {
			for (const entry of readdirSync(p)) walk(join(p, entry));
		} else if (s.isFile()) {
			bytes += s.size;
		}
	};
	walk(path);
	if (bytes > 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	if (bytes > 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${bytes} B`;
}

function ensureCacheDir(): void {
	mkdirSync(BUNDLE_CACHE, { recursive: true });
}

// Pure JS SHA256 (avoid shelling to `shasum` which isn't on Windows by default).
function sha256OfFile(path: string): string {
	const data = readFileSync(path);
	return createHash("sha256").update(data).digest("hex");
}

function downloadAndVerify(
	url: string,
	dest: string,
	expectedSha256: string,
): void {
	if (existsSync(dest)) {
		const actual = sha256OfFile(dest);
		if (actual === expectedSha256) return;
		console.warn(
			`[stage-vendor-windows] cached ${dest} has wrong sha256 (got ${actual}); re-downloading`,
		);
		rmSync(dest, { force: true });
	}
	console.log(`[stage-vendor-windows] downloading ${url}`);
	mkdirSync(dirname(dest), { recursive: true });
	// curl ships with Windows 10+ as curl.exe.
	execFileSync("curl.exe", ["-fL", "--retry", "3", "-o", dest, url], {
		stdio: "inherit",
	});
	const actual = sha256OfFile(dest);
	if (actual !== expectedSha256) {
		rmSync(dest, { force: true });
		throw new Error(
			`[stage-vendor-windows] sha256 mismatch for ${url}\n  expected: ${expectedSha256}\n  actual:   ${actual}`,
		);
	}
}

function freshExtractDir(path: string): void {
	rmSync(path, { recursive: true, force: true });
	mkdirSync(path, { recursive: true });
}

function unzip(archive: string, dest: string): void {
	// Use PowerShell's Expand-Archive — it ships with Windows.
	// -Force lets it overwrite an existing dest (we always pass a fresh dir).
	execFileSync(
		"powershell.exe",
		[
			"-NoProfile",
			"-NonInteractive",
			"-Command",
			`Expand-Archive -LiteralPath '${archive}' -DestinationPath '${dest}' -Force`,
		],
		{ stdio: "inherit" },
	);
}

// ---------------------------------------------------------------------------
// claude-code — copy from the already-installed platform sub-package.
//
// We do NOT support cross-arch staging from Windows host yet — building Windows
// installers on Windows is the only supported flow. (Upstream macOS code
// supports cross-arch via npm tarball download; we can mirror that later if CI
// ever needs it.)
// ---------------------------------------------------------------------------

function stageClaudeCodeBinary(target: TargetInfo): string {
	const installed = join(NODE_MODULES, target.claudeCodePkg, "claude.exe");
	ensureExists(installed, `${target.claudeCodePkg}/claude.exe`);
	const dest = join(DIST_VENDOR, "claude-code", "claude.exe");
	copyFile(installed, dest);
	return dest;
}

// ---------------------------------------------------------------------------
// codex — copy the entire `codex/` and `path/` dirs from the platform sub-pkg.
//
// Layout in node_modules:
//   @openai/codex-win32-x64/vendor/<triple>/
//     codex/codex.exe                       ← main
//     codex/codex-command-runner.exe        ← Windows-only helper
//     codex/codex-windows-sandbox-setup.exe ← Windows-only helper
//     path/rg.exe                           ← ripgrep on PATH at runtime
//
// Output:
//   dist/vendor/codex/codex.exe
//   dist/vendor/codex/codex-command-runner.exe
//   dist/vendor/codex/codex-windows-sandbox-setup.exe
//   dist/vendor/codex/path/rg.exe
//
// helmor-sidecar prepends `dist/vendor/codex/path/` to codex child PATH at
// runtime so codex can find ripgrep without it being globally installed.
// ---------------------------------------------------------------------------

function stageCodexBinary(target: TargetInfo): void {
	const archRoot = join(
		NODE_MODULES,
		target.codexPkg,
		"vendor",
		target.codexTriple,
	);
	const codexSrc = join(archRoot, "codex");
	const pathSrc = join(archRoot, "path");
	ensureExists(join(codexSrc, "codex.exe"), `${target.codexPkg}/codex/codex.exe`);

	const codexDest = join(DIST_VENDOR, "codex");
	mkdirSync(codexDest, { recursive: true });
	cpSync(codexSrc, codexDest, { recursive: true });

	if (existsSync(pathSrc)) {
		const pathDest = join(DIST_VENDOR, "codex", "path");
		cpSync(pathSrc, pathDest, { recursive: true });
	}
}

// ---------------------------------------------------------------------------
// gh — download zip + extract bin/gh.exe.
// ---------------------------------------------------------------------------

function locateExtractedExe(extractDir: string, name: string): string {
	const direct = join(extractDir, "bin", `${name}.exe`);
	if (existsSync(direct)) return direct;
	for (const entry of readdirSync(extractDir)) {
		const nested = join(extractDir, entry, "bin", `${name}.exe`);
		if (existsSync(nested)) return nested;
	}
	throw new Error(
		`[stage-vendor-windows] could not locate bin/${name}.exe under ${extractDir}`,
	);
}

function stageGhBinary(arch: "amd64" | "arm64"): string {
	ensureCacheDir();
	const slug = `gh_${GH_VERSION}_windows_${arch}`;
	const archive = join(BUNDLE_CACHE, `${slug}.zip`);
	const url = `https://github.com/cli/cli/releases/download/v${GH_VERSION}/${slug}.zip`;
	downloadAndVerify(url, archive, GH_SHA256[arch]);

	const extractDir = join(BUNDLE_CACHE, slug);
	freshExtractDir(extractDir);
	unzip(archive, extractDir);

	const binSrc = locateExtractedExe(extractDir, "gh");
	const binDest = join(DIST_VENDOR, "gh", "gh.exe");
	copyFile(binSrc, binDest);
	return binDest;
}

// ---------------------------------------------------------------------------
// glab — download zip + extract bin/glab.exe.
//
// NOTE: GitLab v1.93.0 ships glab Windows release as a zip whose root contains
// `bin/glab.exe` directly (no nested arch wrapper dir, unlike the macOS
// tar.gz). We still pass through locateExtractedExe so wrapper-dir layouts in
// future releases keep working.
// ---------------------------------------------------------------------------

function stageGlabBinary(arch: "amd64"): string {
	ensureCacheDir();
	const slug = `glab_${GLAB_VERSION}_windows_${arch}`;
	const archive = join(BUNDLE_CACHE, `${slug}.zip`);
	const url = `https://gitlab.com/gitlab-org/cli/-/releases/v${GLAB_VERSION}/downloads/${slug}.zip`;
	downloadAndVerify(url, archive, GLAB_SHA256[arch]);

	const extractDir = join(BUNDLE_CACHE, slug);
	freshExtractDir(extractDir);
	unzip(archive, extractDir);

	const binSrc = locateExtractedExe(extractDir, "glab");
	const binDest = join(DIST_VENDOR, "glab", "glab.exe");
	copyFile(binSrc, binDest);
	return binDest;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const target = detectTarget();

console.log(
	`[stage-vendor-windows] host=win32/${process.arch} target=win32/${target.arch} (${target.codexTriple})`,
);

rmSync(DIST_VENDOR, { recursive: true, force: true });
mkdirSync(DIST_VENDOR, { recursive: true });

stageClaudeCodeBinary(target);
stageCodexBinary(target);
stageGhBinary(target.ghArch);
stageGlabBinary(target.glabArch);

console.log(`[stage-vendor-windows] ✓ staged → ${DIST_VENDOR}`);
console.log(`  claude-code ${humanSize(join(DIST_VENDOR, "claude-code"))}`);
console.log(`  codex       ${humanSize(join(DIST_VENDOR, "codex"))}`);
console.log(`  gh          ${humanSize(join(DIST_VENDOR, "gh"))}`);
console.log(`  glab        ${humanSize(join(DIST_VENDOR, "glab"))}`);
