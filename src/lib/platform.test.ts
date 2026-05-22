import { afterEach, describe, expect, it, vi } from "vitest";

describe("platform", () => {
	afterEach(() => {
		vi.unstubAllGlobals();
		vi.resetModules();
	});

	it("detects Windows from userAgent", async () => {
		vi.stubGlobal("navigator", {
			userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
		});
		const { getPlatform, isMac, isWindows } = await import("./platform");
		expect(getPlatform()).toBe("windows");
		expect(isWindows()).toBe(true);
		expect(isMac()).toBe(false);
	});

	it("detects macOS from userAgent", async () => {
		vi.stubGlobal("navigator", {
			userAgent:
				"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
		});
		const { getPlatform, isMac, isWindows } = await import("./platform");
		expect(getPlatform()).toBe("macos");
		expect(isMac()).toBe(true);
		expect(isWindows()).toBe(false);
	});
});
