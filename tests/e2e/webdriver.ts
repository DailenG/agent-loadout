// A minimal W3C WebDriver client for the end-to-end smoke tests: just the
// endpoints they use, over fetch, so the e2e leg needs no browser
// automation package. tauri-driver (https://v2.tauri.app/develop/tests/webdriver/)
// serves the protocol and starts the app through the platform's native
// driver (msedgedriver for WebView2, WebKitWebDriver on Linux).

import { setTimeout as sleep } from "node:timers/promises";

const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

type WebDriverValue = { value: unknown };

async function call(url: string, method: "GET" | "POST" | "DELETE", body?: unknown) {
  const response = await fetch(url, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = (await response.json()) as WebDriverValue;
  if (!response.ok) {
    const error = json.value as { error?: string; message?: string };
    throw new Error(`${method} ${url}: ${error.error ?? response.status} ${error.message ?? ""}`);
  }
  return json.value;
}

export class Session {
  private readonly base: string;
  readonly id: string;

  private constructor(base: string, id: string) {
    this.base = base;
    this.id = id;
  }

  /** Starts the app at `application` through tauri-driver at `server`. */
  static async start(server: string, application: string): Promise<Session> {
    const value = (await call(`${server}/session`, "POST", {
      capabilities: {
        alwaysMatch: {
          browserName: "wry",
          "tauri:options": { application },
        },
      },
    })) as { sessionId: string };
    return new Session(`${server}/session/${value.sessionId}`, value.sessionId);
  }

  async quit() {
    await call(this.base, "DELETE");
  }

  /** The viewport as PNG bytes. */
  async screenshot(): Promise<Buffer> {
    const base64 = (await call(`${this.base}/screenshot`, "GET")) as string;
    return Buffer.from(base64, "base64");
  }

  /** The first element matching an XPath expression, waiting up to `timeoutMs`. */
  async find(xpath: string, timeoutMs = 15_000): Promise<string> {
    const deadline = Date.now() + timeoutMs;
    let lastError: unknown;
    while (Date.now() < deadline) {
      try {
        const value = (await call(`${this.base}/element`, "POST", {
          using: "xpath",
          value: xpath,
        })) as Record<string, string>;
        // W3C key, or the legacy JSON Wire key some drivers still send.
        const element = value[ELEMENT_KEY] ?? value.ELEMENT;
        if (element) {
          return element;
        }
        lastError = `unexpected element reply ${JSON.stringify(value)}`;
        await sleep(250);
      } catch (error) {
        lastError = error;
        await sleep(250);
      }
    }
    throw new Error(`no element for ${xpath}: ${String(lastError)}`);
  }

  async click(xpath: string) {
    const element = await this.find(xpath);
    await call(`${this.base}/element/${element}/click`, "POST", {});
  }

  async type(xpath: string, text: string) {
    const element = await this.find(xpath);
    await call(`${this.base}/element/${element}/clear`, "POST", {});
    await call(`${this.base}/element/${element}/value`, "POST", { text });
  }

  async text(xpath: string): Promise<string> {
    const element = await this.find(xpath);
    return (await call(`${this.base}/element/${element}/text`, "GET")) as string;
  }

  /** Whether a checkbox, radio or option is selected. */
  async selected(xpath: string): Promise<boolean> {
    const element = await this.find(xpath);
    return (await call(`${this.base}/element/${element}/selected`, "GET")) as boolean;
  }

  /** The `aria-label` of every element matching `xpath` (diagnostics). */
  async labels(xpath: string): Promise<string[]> {
    const found = (await call(`${this.base}/elements`, "POST", {
      using: "xpath",
      value: xpath,
    })) as Record<string, string>[];
    return Promise.all(
      found.map(async (value) => {
        const element = value[ELEMENT_KEY] ?? value.ELEMENT;
        return (await call(`${this.base}/element/${element}/attribute/aria-label`, "GET")) as string;
      }),
    );
  }
}

/** Polls `check` until it returns true or `timeoutMs` passes. */
export async function waitFor(check: () => boolean | Promise<boolean>, what: string, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await check()) {
      return;
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${what}`);
}
