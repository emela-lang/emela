---
title: Bytes at the boundary of the native target
authors: []
status: implemented
issue:
touches: [ext.boundary, ext.c-host, target.native.export, diag.list]
requires: []
related: [bytes, tcp, native-target]
pr: 877
decided: 2026-11-25
---

# Bytes at the boundary of the native target

## Summary

Give `Bytes` a C form at the boundary of external functions and exported functions on the native target: a parameter is a pointer and a length, a return value is a struct `em_bytes` of a pointer allocated with `malloc` and a length, and an external `suspend fn` completes with `em_complete_bytes`. E0443 then covers only Array. The C form of Bytes is decided apart from callbacks (`[open.list]` #38). For an external `suspend fn`, the bytes of a Bytes or String parameter stay valid until the task completes, not only until the C function returns.

## Motivation

- `[api.std.tcp]` says `SystemTcp` uses POSIX sockets on native, and only `upgrade_tls` has no native implementation. But `read` returns `Option[Bytes]` and `write` takes `Bytes`, and an external function that takes or returns Bytes is E0443 on native (`[ext.boundary]`). `std/tcp.c` therefore has only the registry, and `connect`, `read`, `write` and `close` cannot be bound to C.
- The same block applies to every later platform handler that passes binary data on native (files, random bytes, HTTP bodies).
- `[ext.boundary]` ties the C form of Bytes to callbacks (#38). The questions of #38 are how long the C side may keep a function and its context, and how it reports that it no longer keeps it. A Bytes has none of these: it is an immutable value copied when it is received, as a String is. Nothing in #38 has to be decided first.

## Proposal

### Syntax

No change.

### Meaning

The table of C types in `[ext.boundary]` gets a row for Bytes:

| Emela type | C parameter | C return value |
|---|---|---|
| Bytes | `const uint8_t *data, size_t len` (two parameters) | `em_bytes` |

```c
typedef struct em_bytes {
  uint8_t *data;
  size_t len;
} em_bytes;
```

- `em_bytes` is declared in `emela.h`. A `.c` file whose host function returns Bytes includes it.
- A Bytes parameter takes two C parameters in that position, the pointer to its first byte and the number of bytes. For the empty Bytes, `len` is 0 and `data` MAY be NULL.
- The bytes pointed to by `data` of a parameter are owned by the runtime. The host MUST NOT modify or free them. They are valid until the host function returns, and for an external function declared as `suspend fn`, until the task completes (`[ext.c-host]`). Whether the runtime passes its own bytes or a copy is implementation-defined.
- A returned `em_bytes` is copied into a Bytes, and `data` is freed with `free` right after the copy. `data` is NULL only when `len` is 0; a NULL `data` with a `len` greater than 0 is a defect (``external function `f` returned NULL``).
- A `len` greater than the largest Int (2^31 - 1) is a defect (``external function `f` returned more bytes than a Bytes can hold``). The runtime frees `data` in this case too.
- An external `suspend fn` that returns Bytes completes with `em_complete_bytes(em_task *task, uint8_t *data, size_t len)`. It can be called from any thread. The runtime copies the bytes on its own thread and then calls `free` on `data`. NULL and the length are treated as for a returned `em_bytes`.
- An exported fn (`[target.native.export]`) that takes Bytes receives `const uint8_t *data, size_t len` from the host and copies the bytes. A NULL `data` with a `len` greater than 0 is a defect of the launch (``exported function `f` was given NULL``), and so is a `len` greater than 2^31 - 1 (``exported function `f` was given more bytes than a Bytes can hold``). An exported fn that returns Bytes returns an `em_bytes` whose `data` is allocated with `malloc` (or NULL when `len` is 0), which the host frees.

The pointer of a String parameter (`[ext.boundary]`) follows the same rule: for an external function declared as `suspend fn`, it is valid until the task completes, instead of only until the C function returns. A task that is canceled still completes (`[ext.c-host]`), so the bytes are valid until that completion.

E0443 no longer covers Bytes. It covers a type that contains an Array, so `Array[Bytes]` is still E0443.

### Typing rules

No change.

## Diagnostics

- [x] E0443: the description and the examples drop Bytes. The message for Bytes is no longer produced.
- [x] No new codes. The new defects (a NULL `data` with a nonzero length, a length above 2^31 - 1) are runtime defects with messages, not diagnostics.

## Compatibility

Programs that compiled keep their meaning. A host function written for the String rule before (copying the string before it returns) stays correct; the longer validity only allows more. Programs that were E0443 because of Bytes alone now compile on native. `emela.h` gains `em_bytes` and `em_complete_bytes`; its signatures can change between versions (`[ext.c-host]`).

## Implementation

- A Bytes in the C runtime is an `EM_KIND_STRING` object with its length in slot 0, so a parameter passes the address of its bytes and the length without a copy.
- Keeping the bytes of a Bytes or String parameter valid until a suspended task completes is one more reference held by the task record, dropped on completion. A String that the runtime copied to add the NUL is kept the same way.
- `em_complete_bytes` stores the pointer and length in the task record, as `em_complete_string` stores its string.
- After this, `std/tcp.c` binds `connect`, `read`, `write` and `close` (`upgrade_tls` stays E0412 on native).

## Alternatives

- Return Bytes through an out parameter (`size_t host(..., uint8_t **out)`). It keeps C functions returning a scalar, but the shape of the prototype differs between Bytes and every other return type, and the parameter list no longer matches the declaration.
- Pass a parameter as `em_bytes` by value instead of two parameters. It is one struct for both directions, but `data` of a parameter is not the host's to free, and the field would have to be `const` in one direction and not in the other.
- Let the host allocate the result with a runtime function (`em_bytes_alloc(len)`) and fill it, saving a copy. Every `em_` function other than completion is limited to the runtime's thread (`[ext.c-host]`), and a host that reads on another thread could not use it. The copy can be removed later without changing what the host writes.
- Keep a parameter of a `suspend fn` valid only until the host function returns, as String was. `write` of Tcp on a non-blocking socket hands the bytes over across several `em_on_writable` callbacks, so the host would copy every write. Bytes and String would then also follow different rules.
- Extend the validity for Bytes only, and leave String as it is. The two are the same kind of object in the C runtime, and two rules for the same situation have to be remembered apart.
- Wait for #38 and decide Bytes and Array with callbacks. Nothing in #38 constrains Bytes, and Std.Tcp waits meanwhile.

## Prior art

## Open questions

## Decision

Bytes の C 形は callbacks（#38）に依存しないので切り離す．Std.Tcp の native 実装をこれで進められる．String の有効期限も suspend fn ではタスク完了までに揃える．
