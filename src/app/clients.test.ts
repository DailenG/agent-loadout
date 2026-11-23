import { describe, expect, it } from "vitest";
import { RECOMMENDED_CLIENT, orderByRecommended, recommendedFirst } from "./clients";

const id = (item: { id: string }) => item.id;
const items = (...ids: string[]) => ids.map((value) => ({ id: value }));

describe("orderByRecommended", () => {
  it("moves the recommended client first and keeps the others in their order", () => {
    const ordered = orderByRecommended(items("claude-code", "codex", RECOMMENDED_CLIENT, "pi", "cursor"), id);
    expect(ordered.map(id)).toEqual([RECOMMENDED_CLIENT, "claude-code", "codex", "pi", "cursor"]);
  });

  it("is a same-order copy without the recommended client", () => {
    const input = items("claude-code", "codex", "pi");
    const ordered = orderByRecommended(input, id);
    expect(ordered).toEqual(input);
    expect(ordered).not.toBe(input);
  });

  it("leaves a list that already starts with the recommended client alone", () => {
    expect(orderByRecommended(items(RECOMMENDED_CLIENT, "codex"), id).map(id)).toEqual([RECOMMENDED_CLIENT, "codex"]);
    expect(orderByRecommended([], id)).toEqual([]);
  });
});

describe("recommendedFirst", () => {
  it("breaks ties after a list's own sort key", () => {
    const tools = [
      { id: "cursor", detected: false },
      { id: "claude-code", detected: true },
      { id: RECOMMENDED_CLIENT, detected: false },
      { id: "codex", detected: true },
    ];
    const tie = recommendedFirst(id);
    const sorted = [...tools].sort((left, right) => Number(right.detected) - Number(left.detected) || tie(left, right));
    expect(sorted.map(id)).toEqual(["claude-code", "codex", RECOMMENDED_CLIENT, "cursor"]);
  });
});
