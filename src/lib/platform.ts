/**
 * Runtime platform helpers for UI chrome (shortcuts, window controls).
 *
 * Helmor ships macOS and Windows builds; `navigator.userAgent` in the Tauri
 * webview reflects the host OS. Vitest may stub `navigator`; some tests mock
 * this module to pin macOS shortcut glyphs.
 */

export type HelmorPlatform = "macos" | "windows" | "linux" | "unknown";

function detectPlatform(): HelmorPlatform {
	if (typeof navigator === "undefined") {
		return "unknown";
	}
	const ua = navigator.userAgent;
	if (/Windows/i.test(ua)) {
		return "windows";
	}
	if (/Macintosh|Mac OS X/i.test(ua)) {
		return "macos";
	}
	if (/Linux/i.test(ua)) {
		return "linux";
	}
	return "unknown";
}

const platform = detectPlatform();

export function getPlatform(): HelmorPlatform {
	return platform;
}

export function isMac(): boolean {
	return platform === "macos";
}

export function isWindows(): boolean {
	return platform === "windows";
}
