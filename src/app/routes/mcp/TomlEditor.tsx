// CodeMirror 6 editor for a server's TOML file (SPEC 13.2 TOML tab). The
// document is owned by the editor; every change is reported via `onChange`.

import { StreamLanguage } from "@codemirror/language";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { useEffect, useRef } from "react";

type TomlEditorProps = {
  /** The initial document; later changes of this prop are ignored. */
  value: string;
  onChange: (text: string) => void;
  ariaLabel: string;
};

const tomlLanguage = StreamLanguage.define(toml);

export function TomlEditor({ value, onChange, ariaLabel }: TomlEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const initialValue = useRef(value);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initialValue.current,
        extensions: [
          basicSetup,
          tomlLanguage,
          EditorView.contentAttributes.of({ "aria-label": ariaLabel, spellcheck: "false" }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              onChangeRef.current(update.state.doc.toString());
            }
          }),
        ],
      }),
    });
    return () => view.destroy();
  }, [ariaLabel]);

  return <div ref={hostRef} className="mcp-toml-editor" />;
}
