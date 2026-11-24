import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ScanFindings } from "./ScanFindings";

describe("ScanFindings", () => {
  it("shows custom rules, rules warnings, and where each finding is", () => {
    render(
      <ScanFindings
        scan={{
          verdict: "warning",
          customRules: true,
          warnings: ["scan-rules.toml: rule `x` has no pattern"],
          rulesVersion: 2,
          cached: false,
          findings: [
            {
              scanner: "builtin",
              rule: "mcp-unpinned-package",
              severity: "warning",
              file: null,
              line: null,
              message: "Not pinned.",
              excerpt: "npx -y pkg",
            },
            {
              scanner: "builtin",
              rule: "not-content-scanned",
              severity: "info",
              file: null,
              line: null,
              message: "Read by path only: logo.png",
              excerpt: null,
            },
          ],
        }}
      />,
    );
    expect(screen.getByText("Custom rules in use")).toBeInTheDocument();
    expect(screen.getByText(/rule `x` has no pattern/)).toBeInTheDocument();
    const [command, note] = screen.getAllByRole("listitem");
    expect(within(command).getByText("command line")).toBeInTheDocument();
    expect(within(note).queryByText("command line")).toBeNull();
    expect(within(note).getByText("Read by path only: logo.png")).toBeInTheDocument();
  });
});
