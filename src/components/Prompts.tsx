// Questions a run waits on: a choice between actions, the execution
// warnings, and values for variables the script does not declare. Each is
// asked as a promise, so the code that asks reads top to bottom; `prompts`
// renders whichever are open, above the other dialogs.
import { useState, type ReactNode } from "react";
import type { Dialect, VariableValue } from "../types";
import { VariablesDialog } from "./VariablesDialog";

export interface ChoiceOption {
  label: string;
  /** "" for the option that cancels (also Escape). */
  value: string;
  kind?: "primary" | "danger";
}

interface Choice {
  title: string;
  message: string;
  options: ChoiceOption[];
  resolve: (value: string) => void;
}

interface Warnings {
  messages: string[];
  resolve: (run: boolean) => void;
}

interface Variables {
  names: string[];
  dialect: Dialect;
  resolve: (values: VariableValue[] | null) => void;
}

export interface Prompts {
  /** The value of the option picked; "" when cancelled. */
  askChoice: (title: string, message: string, options: ChoiceOption[]) => Promise<string>;
  /** Whether to run despite the warnings. */
  confirmWarnings: (messages: string[]) => Promise<boolean>;
  /** Values for the variables; null when cancelled. */
  askVariables: (names: string[], dialect: Dialect) => Promise<VariableValue[] | null>;
  prompts: ReactNode;
}

export function usePrompts(): Prompts {
  const [choice, setChoice] = useState<Choice | null>(null);
  const [warnings, setWarnings] = useState<Warnings | null>(null);
  const [variables, setVariables] = useState<Variables | null>(null);

  const askChoice = (title: string, message: string, options: ChoiceOption[]) =>
    new Promise<string>((resolve) =>
      setChoice({
        title,
        message,
        options,
        resolve: (value) => {
          setChoice(null);
          resolve(value);
        },
      }),
    );
  const confirmWarnings = (messages: string[]) =>
    new Promise<boolean>((resolve) =>
      setWarnings({
        messages,
        resolve: (run) => {
          setWarnings(null);
          resolve(run);
        },
      }),
    );
  const askVariables = (names: string[], dialect: Dialect) =>
    new Promise<VariableValue[] | null>((resolve) =>
      setVariables({
        names,
        dialect,
        resolve: (values) => {
          setVariables(null);
          resolve(values);
        },
      }),
    );

  const prompts = (
    <>
      {choice && (
        <div className="modal-backdrop">
          <div className="modal confirm-modal" onKeyDown={(e) => e.key === "Escape" && choice.resolve("")}>
            <div className="modal-title">{choice.title}</div>
            <div className="confirm-body">
              <p>{choice.message}</p>
            </div>
            <div className="modal-actions">
              <span className="spacer" />
              {choice.options.map((o) => (
                <button key={o.label} className={o.kind ?? ""} autoFocus={o.value === ""} onClick={() => choice.resolve(o.value)}>
                  {o.label}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
      {variables && (
        <VariablesDialog
          names={variables.names}
          dialect={variables.dialect}
          onRun={(v) => variables.resolve(v)}
          onCancel={() => variables.resolve(null)}
        />
      )}
      {warnings && (
        <div className="modal-backdrop">
          <div className="modal confirm-modal" onKeyDown={(e) => e.key === "Escape" && warnings.resolve(false)}>
            <div className="modal-title">⚠ Execution warning</div>
            <div className="confirm-body">
              {warnings.messages.map((m, i) => (
                <div key={i}>{m}</div>
              ))}
              <p>Do you want to run it anyway?</p>
            </div>
            <div className="modal-actions">
              <span className="spacer" />
              <button className="danger" onClick={() => warnings.resolve(true)}>
                Execute anyway
              </button>
              <button className="primary" autoFocus onClick={() => warnings.resolve(false)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
  return { askChoice, confirmWarnings, askVariables, prompts };
}
