// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/features/skills/components/SkillDiffView.tsx: the
// CodeMirror unified merge view of a file against its other version and the end-of-file anchor
// that keeps a last line without a newline from showing as changed. Read only here (an update
// preview, SPEC 12.4); SkillDock's hunk revert and editing are not ported.

import { unifiedMergeView } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { minimalSetup } from "codemirror";
import { useEffect, useRef } from "react";

type SkillDiffViewProps = {
  /** The file as it is now (the store copy); empty when added. */
  before: string;
  /** The file as the update would make it; empty when removed. */
  after: string;
  ariaLabel: string;
};

/** Both sides with a trailing newline when neither has one (SkillDock's EOF anchor). */
export function diffDocuments(before: string, after: string) {
  const anchor = before.length > 0 && after.length > 0 && !before.endsWith("\n") && !after.endsWith("\n");
  return anchor ? { original: `${before}\n`, current: `${after}\n` } : { original: before, current: after };
}

function renderMergeControl() {
  const hidden = document.createElement("span");
  hidden.hidden = true;
  return hidden;
}

export function SkillDiffView({ before, after, ariaLabel }: SkillDiffViewProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const { original, current } = diffDocuments(before, after);
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: current,
        extensions: [
          minimalSetup,
          lineNumbers(),
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({ "aria-label": ariaLabel }),
          unifiedMergeView({
            original,
            highlightChanges: true,
            gutter: true,
            mergeControls: renderMergeControl,
          }),
        ],
      }),
    });
    return () => view.destroy();
  }, [after, ariaLabel, before]);

  return (
    <div className="skill-diff">
      <div ref={hostRef} className="skill-diff__editor" />
    </div>
  );
}
