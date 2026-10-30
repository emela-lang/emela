typedef struct {
  uint32_t slot;
  uint32_t generation;
} EmelaTask;

/* May be called from an ISR. Copies a completion into the fixed event queue.
 * Returns zero for a stale, duplicate, or mismatched completion; never resumes Emela here.
 * A full event queue calls the board defect hook after restoring the interrupt state. */
int emela_task_complete_unit(EmelaTask task);
int emela_task_complete_int(EmelaTask task, int32_t value);
int emela_task_complete_int64(EmelaTask task, int64_t value);
int emela_task_complete_float(EmelaTask task, double value);
/* Start-adapter arguments are borrowed only for the duration of the start call.
 * For later hardware use, snapshot them into board-owned storage before returning.
 * Retain managed containers explicitly. For ownerless String views, copy exactly length
 * bytes into board storage (embedded NUL bytes are valid); retaining NULL owner does nothing.
 * Never keep pointers into caller frames after start, including after cancellation.
 */
