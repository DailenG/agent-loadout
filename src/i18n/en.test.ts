import { describe, expect, it } from "vitest";
import { failureSentence } from "./en";

describe("failureSentence", () => {
  it("shows the core's one Git-missing sentence instead of the generic text", () => {
    const message = "Git isn't installed or isn't on PATH; install Git to use Git remotes.";
    expect(failureSentence({ code: "tool_not_found", message })).toBe(message);
  });

  it("keeps the generic text for any other missing or failing program (diagnostic detail is never shown)", () => {
    for (const message of [
      "cannot run git: Access is denied. (os error 5)",
      "the agents `skills` CLI is not installed or not on PATH; Loadout never runs it through npx",
      "cannot start xdg-open: No such file or directory",
    ]) {
      expect(failureSentence({ code: "tool_not_found", message })).toBe("A required program is not installed or not on PATH.");
    }
  });
});
