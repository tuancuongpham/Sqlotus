import { useEffect, useRef } from "react";
import { monaco } from "../monaco";

interface Props {
  title: string;
  originalLabel: string;
  modifiedLabel: string;
  original: string;
  modified: string;
  language: string;
  onClose: () => void;
}

/** Side-by-side diff of two texts (history versions). */
export function DiffDialog({ title, originalLabel, modifiedLabel, original, modified, language, onClose }: Props) {
  const el = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const left = monaco.editor.createModel(original, language);
    const right = monaco.editor.createModel(modified, language);
    const diff = monaco.editor.createDiffEditor(el.current!, {
      automaticLayout: true,
      readOnly: true,
      renderSideBySide: true,
      minimap: { enabled: false },
      fontSize: 13,
    });
    diff.setModel({ original: left, modified: right });
    return () => {
      diff.dispose();
      left.dispose();
      right.dispose();
    };
  }, [original, modified, language]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal diff-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">{title}</div>
        <div className="diff-labels">
          <span>{originalLabel}</span>
          <span>{modifiedLabel}</span>
        </div>
        <div className="diff-editor" ref={el} />
        <div className="modal-actions">
          <span className="spacer" />
          <button className="primary" onClick={onClose} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
