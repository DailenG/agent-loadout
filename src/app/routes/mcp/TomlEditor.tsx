// CodeMirror 6 editor for a server's TOML file (SPEC 13.2 TOML tab). The
// document follows `value`; every edit is reported via `onChange`.

import { StreamLanguage } from "@codemirror/language";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { useEffect, useRef } from "react";

type TomlEditorProps = {
  /** The document; a new value replaces the editor's text. */
  value: string;
  onChange: (text: string) => void;
  ariaLabel: string;
};

const tomlLanguage = StreamLanguage.define(toml);

export function TomlEditor({ value, onChange, ariaLabel }: TomlEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const valueRef = useRef(value);
  valueRef.current = value;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: valueRef.current,
        extensions: [
          basicSetup,
          tomlLanguage,
          EditorView.contentAttributes.of({ "aria-label": ariaLabel, spellcheck: "false" }),
          EditorView.updateListener.of((update) => {
            const text = update.state.doc.toString();
            // A new `value` set from outside is not an edit.
            if (update.docChanged && text !== valueRef.current) {
              onChangeRef.current(text);
            }
          }),
        ],
      }),
    });
    viewRef.current = view;
    return () => {
      viewRef.current = null;
      view.destroy();
    };
  }, [ariaLabel]);

  useEffect(() => {
    const view = viewRef.current;
    if (view && view.state.doc.toString() !== value) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
    }
  }, [value]);

  return <div ref={hostRef} className="mcp-toml-editor" />;
}
