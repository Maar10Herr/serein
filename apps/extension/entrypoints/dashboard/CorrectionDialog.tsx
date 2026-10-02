import { useState } from "preact/hooks";
import { Modal } from "../../lib/Modal";
import type { DashboardCard } from "../../lib/types";
import type { MessageKey } from "../../lib/locales";

export type CorrectionSubmission = Readonly<{
  atom_id: string;
  action: string;
  text: string | null;
  dialog_generation: number;
}>;

type CorrectionDialogProps = {
  card: DashboardCard;
  generation: number;
  returnFocusTo?: HTMLElement | null;
  translate: (key: MessageKey) => string;
  isCurrentGeneration: (generation: number) => boolean;
  onClose: (generation: number) => void;
  onSubmit: (submission: CorrectionSubmission) => Promise<void>;
  onRefresh: () => void;
  onSaved: (generation: number) => void;
};

const actions = [
  ["not_about_me", "correction.notAboutMe"],
  ["wrong_topic", "correction.wrongTopic"],
  ["temporary_research", "correction.temporaryResearch"],
  ["do_not_use", "correction.doNotUse"],
] as const;

export function CorrectionDialog({
  card,
  generation,
  returnFocusTo,
  translate,
  isCurrentGeneration,
  onClose,
  onSubmit,
  onRefresh,
  onSaved,
}: CorrectionDialogProps) {
  const [draft, setDraft] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");

  async function submit(action: string, text: string | null) {
    if (submitting) return;
    const submission = Object.freeze({
      atom_id: card.id,
      action,
      text,
      dialog_generation: generation,
    });
    setSubmitting(true);
    setError("");

    try {
      await onSubmit(submission);
      onRefresh();
      if (isCurrentGeneration(submission.dialog_generation))
        onSaved(submission.dialog_generation);
    } catch (cause) {
      if (isCurrentGeneration(submission.dialog_generation)) {
        const message = cause instanceof Error ? cause.message : "";
        setError(message || translate("errors.generic"));
      }
    } finally {
      if (isCurrentGeneration(submission.dialog_generation))
        setSubmitting(false);
    }
  }

  return (
    <Modal
      title={translate("modal.correctTitle")}
      onClose={() => onClose(generation)}
      returnFocusTo={returnFocusTo}
    >
      <div class="stack">
        {actions.map(([action, key]) => (
          <button
            disabled={submitting}
            style={{ justifyContent: "flex-start" }}
            onClick={() => void submit(action, null)}
          >
            {translate(key)}
          </button>
        ))}
        <label class="small">
          {translate("modal.constraintLabel")}
          <textarea
            placeholder={translate("modal.constraintExample")}
            value={draft}
            onInput={(event) => setDraft(event.currentTarget.value)}
            maxLength={512}
          />
        </label>
        {error && (
          <div class="notice error" role="alert">
            {error}
          </div>
        )}
        <button
          class="primary"
          disabled={submitting || !draft.trim()}
          onClick={() => void submit("confirm_constraint", draft)}
        >
          {translate("correction.confirmConstraint")}
        </button>
      </div>
    </Modal>
  );
}
