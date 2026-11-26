import { describe, expect, it } from "vitest";
import { readOnlyTooltip } from "@/app/components/ReadOnlyBadge";
import { displayPath } from "./labels";

describe("displayPath", () => {
  it("names a project path by the project folder, never the <P> placeholder", () => {
    expect(displayPath("<P>/.claude/skills")).toBe(".claude/skills in the project folder");
    expect(displayPath("<P>/.mcp.json", "C:/code/web-shop")).toBe("C:/code/web-shop/.mcp.json");
  });

  it("leaves user and absolute paths as they are", () => {
    expect(displayPath("~/.claude/skills")).toBe("~/.claude/skills");
    expect(displayPath("C:/tools/skills")).toBe("C:/tools/skills");
  });

  it("keeps the placeholder out of read-only tooltips", () => {
    expect(readOnlyTooltip({ kind: "acl" }, "<P>/.mcp.json")).not.toContain("<P>");
  });
});
