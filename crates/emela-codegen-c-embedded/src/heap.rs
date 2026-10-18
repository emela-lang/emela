//! The heap (`docs/design/c-embedded-architecture.md`, 決定: メモリ管理): the values that stay
//! references after value typing (`List[T]`, recursive enums, enums and tuples holding them, cells
//! of them) live in boxes taken from one static area of `heap_bytes` bytes.
//!
//! A box is a struct per type: a reference count first (0 for static boxes, which are never
//! counted or freed), then the fields laid out as a value aggregate's (`List[T]` is a cell
//! `{rc, head, tail}`; the empty list is `NULL`). A box is taken from the free list of its size,
//! or cut from the unused end of the area; when both are empty, the defect `HEAP_EXHAUSTED`.
//! A freed box goes back to the free list of its size (sizes are not reused for other sizes:
//! simple and constant time, at the cost of some waste when a program's use changes). Interrupts
//! never touch the heap, so counting is not atomic.
//!
//! Freeing does not recurse: a box whose count reaches zero waits in a list threaded through the
//! count members of the waiting boxes (its offset and type index in 32 bits), and `em_drain` frees
//! the waiting boxes one by one, adding the boxes they hold that become free. The stack stays the
//! same however deep a list or a tree is.

use emela_core::{Type, TypeId};

/// The layout of a box: the field types, with type arguments substituted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Boxed {
    /// The fields of each constructor.
    Enum(Vec<Vec<TypeId>>),
    Tuple(Vec<TypeId>),
    /// A list cell: the element type (the tail is the list itself).
    List(TypeId),
    /// A cell of a value that is a reference.
    Cell(TypeId),
}

/// The layout of the box of a reference type, if it has one (function values, handlers and
/// evidence tables are not heap values here: static specialization removes them).
pub(crate) fn box_layout(core: &mut emela_core::Module, ty: TypeId) -> Option<Boxed> {
    match core.ty(ty).clone() {
        Type::List(t) => Some(Boxed::List(t)),
        Type::Cell(t) => Some(Boxed::Cell(t)),
        Type::Tuple(ts) => Some(Boxed::Tuple(ts)),
        Type::Enum(id, args) => {
            let def = &core.enums[id];
            if def.external || def.variants.is_empty() {
                return None;
            }
            let variants: Vec<Vec<TypeId>> = def.variants.iter().map(|v| v.tys.clone()).collect();
            Some(Boxed::Enum(
                variants
                    .into_iter()
                    .map(|tys| {
                        tys.into_iter()
                            .map(|t| {
                                if args.is_empty() {
                                    t
                                } else {
                                    core.types.subst(t, &args)
                                }
                            })
                            .collect()
                    })
                    .collect(),
            ))
        }
        _ => None,
    }
}

impl Boxed {
    /// The types of the fields, for finding what a box holds.
    pub(crate) fn field_types(&self, ty: TypeId) -> Vec<TypeId> {
        match self {
            Boxed::Enum(vs) => vs.iter().flatten().copied().collect(),
            Boxed::Tuple(ts) => ts.clone(),
            Boxed::List(t) => vec![*t, ty],
            Boxed::Cell(t) => vec![*t],
        }
    }
}

/// The most box types: a waiting box's type index takes the top 8 bits of its entry.
pub(crate) const MAX_BOX_TYPES: usize = 255;
/// The largest heap: a waiting box's offset (in units of 8 bytes, plus 1) takes the low 24 bits.
pub(crate) const MAX_HEAP_BYTES: u32 = ((1 << 24) - 2) * 8;

/// The allocator, after the box types (`EmAnyBox` is their union, which bounds the sizes).
pub(crate) const HEAP_C: &str = r#"#ifndef EMELA_HEAP_BYTES
#error "EMELA_HEAP_BYTES is not set"
#endif
static _Alignas(8) uint8_t em_heap[EMELA_HEAP_BYTES];
/* The bytes cut from the area, and those of the boxes in use. */
static size_t em_heap_used;
static size_t em_heap_live;
/* The free boxes of each size, in units of 8 bytes. */
static void *em_free_boxes[(sizeof(EmAnyBox) + 7) / 8 + 1];

static void *em_alloc(size_t size) {
  size_t units = (size + 7) / 8;
  void *p = em_free_boxes[units];
  em_heap_live += units * 8;
  if (p != NULL) {
    em_free_boxes[units] = *(void **)p;
    return p;
  }
  if (units * 8 > EMELA_HEAP_BYTES - em_heap_used) {
    emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, "heap exhausted", 14);
  }
  p = em_heap + em_heap_used;
  em_heap_used += units * 8;
  return p;
}

static inline void em_free(void *p, size_t size) {
  size_t units = (size + 7) / 8;
  *(void **)p = em_free_boxes[units];
  em_free_boxes[units] = p;
  em_heap_live -= units * 8;
}

uint32_t emela_heap_used_bytes(void) { return (uint32_t)em_heap_used; }
uint32_t emela_heap_live_bytes(void) { return (uint32_t)em_heap_live; }

/* A box's count is its first member; static boxes (count 0) are not counted. */
static inline void em_dup(void *p) {
  if (p != NULL && *(uint32_t *)p != 0) (*(uint32_t *)p)++;
}

/* Takes one count off a box; whether that was its last. */
static inline bool em_release(void *p) {
  return p != NULL && *(uint32_t *)p != 0 && --*(uint32_t *)p == 0;
}

/* The boxes waiting to be freed: the entry of the first (0 if none), whose count member holds the
 * entry of the next. An entry is the box's offset in units of 8 bytes plus 1, and its type index
 * in the top bits. */
#define EM_PENDING_TYPE_SHIFT 24
#define EM_PENDING_OFFSET ((UINT32_C(1) << EM_PENDING_TYPE_SHIFT) - 1)
static uint32_t em_pending;

static inline void em_defer(void *p, uint32_t type) {
  *(uint32_t *)p = em_pending;
  em_pending = (uint32_t)(((uint8_t *)p - em_heap) / 8 + 1) | (type << EM_PENDING_TYPE_SHIFT);
}
"#;
