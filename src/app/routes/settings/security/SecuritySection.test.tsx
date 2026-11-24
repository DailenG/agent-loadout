import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SecuritySection } from "./SecuritySection";

// Snyk Agent Scan is disabled in this build (0.7.1): Settings has no Security section, even for a machine
// where 0.7.0 installed the scanner and turned it on, and asks the backend nothing about it.
describe("SecuritySection (Snyk Agent Scan, disabled)", () => {
  it("renders nothing and calls no scanner command", async () => {
    const { transport } = renderWithProviders(<SecuritySection />, {
      scanner: { installed: true, enabled: true, tokenRef: "env:SNYK_TOKEN" },
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByText(/Snyk|Security/)).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(transport.calls.filter((call) => call.toLowerCase().includes("scanner"))).toEqual([]);
  });
});
