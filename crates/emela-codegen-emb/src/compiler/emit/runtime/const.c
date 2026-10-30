typedef struct {
  unsigned char status;
  EmelaDefectKind kind;
  const char *message;
  uint16_t length;
} EmelaConstState;

typedef struct EmelaConstContext EmelaConstContext;
struct EmelaConstContext {
  EmelaConstContext *previous;
  EmelaConstState *state;
  const char *prefix;
  uint16_t prefix_length;
};

static EmelaConstContext *emela_const_current;
static char emela_const_diagnostics[EMELA_CONST_DIAGNOSTIC_BYTES];
static uint32_t emela_const_diagnostic_used;
static char emela_const_emergency[EMELA_CONST_EMERGENCY_BYTES];

static _Noreturn void emela_const_defect_sink(EmelaDefectKind kind, const char *message, uint16_t length) {
  emela_board_defect(kind, message, length);
}

/* The macro is private to this generated translation unit, never the board header. */
#define emela_board_defect emela_embedded_report_defect

static inline EMELA_UNUSED void emela_const_copy(char *target, const char *source, uint32_t length) {
  for (uint32_t i = 0; i < length; i++) target[i] = source[i];
}

static inline EMELA_UNUSED uint32_t emela_const_prefix_bytes(void) {
  uint32_t total = 0;
  for (EmelaConstContext *context = emela_const_current; context != NULL; context = context->previous) {
    total += context->prefix_length;
  }
  return total;
}

/* Evaluation cannot resume after the board sink. Reset before a board longjmp as well. */
static _Noreturn void emela_const_self_reference(EmelaDefectKind kind, const char *message, uint16_t length) {
  for (EmelaConstContext *context = emela_const_current; context != NULL; context = context->previous) {
    context->state->status = 0;
  }
  emela_const_current = NULL;
  emela_const_defect_sink(kind, message, length);
}

static _Noreturn void emela_const_diagnostic_exhausted(uint32_t prefixes) {
  static const char reason[] = "const diagnostic capacity exceeded";
  uint32_t offset = prefixes;
  emela_const_copy(emela_const_emergency + offset, reason, sizeof reason - 1);
  for (EmelaConstContext *context = emela_const_current; context != NULL; context = context->previous) {
    offset -= context->prefix_length;
    emela_const_copy(emela_const_emergency + offset, context->prefix, context->prefix_length);
    context->state->status = 0;
  }
  emela_const_current = NULL;
  emela_const_defect_sink(EMELA_DEFECT_STRING_CAPACITY, emela_const_emergency, (uint16_t)(prefixes + sizeof reason - 1));
}

_Noreturn void emela_embedded_report_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  if (emela_const_current == NULL) emela_const_defect_sink(kind, message, length);
  uint32_t prefixes = emela_const_prefix_bytes();
  uint32_t total = prefixes + length;
  if (total > UINT16_MAX || total > EMELA_CONST_DIAGNOSTIC_BYTES - emela_const_diagnostic_used) {
    emela_const_diagnostic_exhausted(prefixes);
  }
  char *target = emela_const_diagnostics + emela_const_diagnostic_used;
  emela_const_diagnostic_used += total;
  uint32_t offset = prefixes;
  emela_const_copy(target + offset, message, length);
  for (EmelaConstContext *context = emela_const_current; context != NULL; context = context->previous) {
    offset -= context->prefix_length;
    emela_const_copy(target + offset, context->prefix, context->prefix_length);
    context->state->kind = kind;
    context->state->message = target + offset;
    context->state->length = (uint16_t)(total - offset);
    context->state->status = 3;
  }
  emela_const_current = NULL;
  emela_const_defect_sink(kind, target, (uint16_t)total);
}
