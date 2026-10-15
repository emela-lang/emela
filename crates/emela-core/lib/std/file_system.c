/* Host functions for SystemFileSystem on C (`@external(c, "./file_system.c", ...)` in
 * file_system.emel), the counterpart of file_system.mjs.
 *
 * The operations are `suspend fn`s (spec 12.6). They do their work synchronously inside the
 * host function with POSIX calls and complete the task at once, so a slow disk stops every
 * fiber for the time of the call (moving the work to a thread is left for later).
 *
 * Like file_system.mjs, an operation gives an outcome instead of failing: its error is "" on
 * success or the name of the Emela error, and each operation maps the host's error codes the
 * same way (`classify`). The message of FileSystemFailure is the error code, the C library's
 * description of it, the call and the path (`EIO: Input/output error, open 'a.txt'`); Node words
 * the description differently. */
#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include "emela.h"

typedef struct emela_fs_outcome {
  const char *error;
  char *message;
  char *text;
  size_t count;
  char **names;
} emela_fs_outcome;

static void *emela_fs_alloc(size_t n) {
  void *p = calloc(1, n == 0 ? 1 : n);
  if (p == NULL) {
    fputs("emela: out of memory\n", stderr);
    abort();
  }
  return p;
}

static char *emela_fs_strdup(const char *s) {
  size_t n = strlen(s);
  char *c = emela_fs_alloc(n + 1);
  memcpy(c, s, n + 1);
  return c;
}

static emela_fs_outcome *emela_fs_ok(void) {
  emela_fs_outcome *o = emela_fs_alloc(sizeof(emela_fs_outcome));
  o->error = "";
  return o;
}

static emela_fs_outcome *emela_fs_failed(const char *error, char *message) {
  emela_fs_outcome *o = emela_fs_ok();
  o->error = error;
  o->message = message;
  return o;
}

static const char *emela_fs_code(int e) {
  switch (e) {
    case ENOENT:
      return "ENOENT";
    case ENOTDIR:
      return "ENOTDIR";
    case EACCES:
      return "EACCES";
    case EPERM:
      return "EPERM";
    case EISDIR:
      return "EISDIR";
    case EEXIST:
      return "EEXIST";
    case ENOTEMPTY:
      return "ENOTEMPTY";
    case ELOOP:
      return "ELOOP";
    case ENAMETOOLONG:
      return "ENAMETOOLONG";
    case EIO:
      return "EIO";
    case ENOSPC:
      return "ENOSPC";
    case EROFS:
      return "EROFS";
    case EBUSY:
      return "EBUSY";
    case EMFILE:
      return "EMFILE";
    case EINVAL:
      return "EINVAL";
    default:
      return "UNKNOWN";
  }
}

/* An extra mapping of an error code to an error name for one operation. */
typedef struct emela_fs_code_map {
  int code;
  const char *error;
} emela_fs_code_map;

/* Turns `errno` of the call `syscall` on `path` into an outcome (file_system.mjs `classify`). */
static emela_fs_outcome *emela_fs_classify(int e, const char *syscall, const char *path,
                                           const emela_fs_code_map *codes, size_t ncodes) {
  for (size_t i = 0; i < ncodes; i++) {
    if (codes[i].code == e) return emela_fs_failed(codes[i].error, emela_fs_strdup(""));
  }
  if (e == ENOENT || e == ENOTDIR) return emela_fs_failed("FileNotFound", emela_fs_strdup(""));
  if (e == EACCES || e == EPERM) return emela_fs_failed("FileAccessDenied", emela_fs_strdup(""));
  const char *code = emela_fs_code(e);
  const char *what = strerror(e);
  size_t n = strlen(code) + strlen(what) + strlen(syscall) + strlen(path) + 16;
  char *message = emela_fs_alloc(n);
  snprintf(message, n, "%s: %s, %s '%s'", code, what, syscall, path);
  return emela_fs_failed("FileSystemFailure", message);
}

/* Whether the bytes are UTF-8 as a fatal WHATWG decoder accepts it (no overlong forms, no
 * surrogates, nothing above U+10FFFF). */
static bool emela_fs_utf8(const unsigned char *s, size_t n) {
  size_t i = 0;
  while (i < n) {
    unsigned char c = s[i];
    if (c < 0x80) {
      i++;
      continue;
    }
    size_t len;
    unsigned char lo = 0x80, hi = 0xBF;
    if (c >= 0xC2 && c <= 0xDF) {
      len = 2;
    } else if (c >= 0xE0 && c <= 0xEF) {
      len = 3;
      if (c == 0xE0) lo = 0xA0;
      if (c == 0xED) hi = 0x9F;
    } else if (c >= 0xF0 && c <= 0xF4) {
      len = 4;
      if (c == 0xF0) lo = 0x90;
      if (c == 0xF4) hi = 0x8F;
    } else {
      return false;
    }
    if (i + len > n) return false;
    if (s[i + 1] < lo || s[i + 1] > hi) return false;
    for (size_t k = 2; k < len; k++) {
      if (s[i + k] < 0x80 || s[i + k] > 0xBF) return false;
    }
    i += len;
  }
  return true;
}

static void emela_fs_complete(em_task *task, emela_fs_outcome *o) { em_complete_handle(task, o); }

void emela_fs_read_text(const char *path, em_task *task) {
  static const emela_fs_code_map codes[] = {{EISDIR, "IsADirectory"}};
  int fd = open(path, O_RDONLY);
  if (fd < 0) {
    emela_fs_complete(task, emela_fs_classify(errno, "open", path, codes, 1));
    return;
  }
  size_t cap = 4096, len = 0;
  char *buf = emela_fs_alloc(cap + 1);
  for (;;) {
    if (len == cap) {
      cap *= 2;
      char *bigger = realloc(buf, cap + 1);
      if (bigger == NULL) {
        fputs("emela: out of memory\n", stderr);
        abort();
      }
      buf = bigger;
    }
    ssize_t got = read(fd, buf + len, cap - len);
    if (got < 0) {
      if (errno == EINTR) continue;
      int e = errno;
      close(fd);
      free(buf);
      emela_fs_complete(task, emela_fs_classify(e, "read", path, codes, 1));
      return;
    }
    if (got == 0) break;
    len += (size_t)got;
  }
  close(fd);
  buf[len] = '\0';
  if (!emela_fs_utf8((const unsigned char *)buf, len)) {
    free(buf);
    emela_fs_complete(task, emela_fs_failed("InvalidText", emela_fs_strdup("")));
    return;
  }
  if (memchr(buf, '\0', len) != NULL) {
    /* A C string cannot carry U+0000 across the host boundary (spec 12.4). */
    free(buf);
    size_t n = strlen(path) + 64;
    char *message = emela_fs_alloc(n);
    snprintf(message, n, "the text of '%s' contains U+0000, which target c cannot read", path);
    emela_fs_complete(task, emela_fs_failed("FileSystemFailure", message));
    return;
  }
  emela_fs_outcome *o = emela_fs_ok();
  o->text = buf;
  emela_fs_complete(task, o);
}

static void emela_fs_write(const char *path, const char *text, int flags, em_task *task) {
  static const emela_fs_code_map codes[] = {{EISDIR, "IsADirectory"}};
  int fd = open(path, O_WRONLY | O_CREAT | flags, 0666);
  if (fd < 0) {
    emela_fs_complete(task, emela_fs_classify(errno, "open", path, codes, 1));
    return;
  }
  size_t len = strlen(text), done = 0;
  while (done < len) {
    ssize_t n = write(fd, text + done, len - done);
    if (n < 0) {
      if (errno == EINTR) continue;
      int e = errno;
      close(fd);
      emela_fs_complete(task, emela_fs_classify(e, "write", path, codes, 1));
      return;
    }
    done += (size_t)n;
  }
  if (close(fd) != 0) {
    emela_fs_complete(task, emela_fs_classify(errno, "close", path, codes, 1));
    return;
  }
  emela_fs_complete(task, emela_fs_ok());
}

void emela_fs_write_text(const char *path, const char *text, em_task *task) {
  emela_fs_write(path, text, O_TRUNC, task);
}

void emela_fs_append_text(const char *path, const char *text, em_task *task) {
  emela_fs_write(path, text, O_APPEND, task);
}

/* False on any error, including a permission error. */
void emela_fs_exists(const char *path, em_task *task) {
  struct stat st;
  em_complete_bool(task, stat(path, &st) == 0);
}

void emela_fs_is_directory(const char *path, em_task *task) {
  struct stat st;
  em_complete_bool(task, stat(path, &st) == 0 && S_ISDIR(st.st_mode));
}

/* UTF-8 byte order is code point order (spec 10.6). */
static int emela_fs_by_name(const void *a, const void *b) {
  return strcmp(*(char *const *)a, *(char *const *)b);
}

void emela_fs_list_directory(const char *path, em_task *task) {
  static const emela_fs_code_map codes[] = {{ENOTDIR, "NotADirectory"}};
  DIR *dir = opendir(path);
  if (dir == NULL) {
    emela_fs_complete(task, emela_fs_classify(errno, "scandir", path, codes, 1));
    return;
  }
  size_t cap = 16, count = 0;
  char **names = emela_fs_alloc(cap * sizeof(char *));
  for (;;) {
    errno = 0;
    struct dirent *entry = readdir(dir);
    if (entry == NULL) {
      if (errno != 0) {
        int e = errno;
        closedir(dir);
        for (size_t i = 0; i < count; i++) free(names[i]);
        free(names);
        emela_fs_complete(task, emela_fs_classify(e, "scandir", path, codes, 1));
        return;
      }
      break;
    }
    if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
    if (count == cap) {
      cap *= 2;
      char **bigger = realloc(names, cap * sizeof(char *));
      if (bigger == NULL) {
        fputs("emela: out of memory\n", stderr);
        abort();
      }
      names = bigger;
    }
    names[count++] = emela_fs_strdup(entry->d_name);
  }
  closedir(dir);
  qsort(names, count, sizeof(char *), emela_fs_by_name);
  emela_fs_outcome *o = emela_fs_ok();
  o->count = count;
  o->names = names;
  emela_fs_complete(task, o);
}

static bool emela_fs_dir(const char *path) {
  struct stat st;
  return stat(path, &st) == 0 && S_ISDIR(st.st_mode);
}

/* Creates the parents too and does nothing if the directory exists (Node's `mkdir` with
 * `recursive`). A file at `path` or in the middle of it is NotADirectory. */
void emela_fs_create_directory(const char *path, em_task *task) {
  static const emela_fs_code_map codes[] = {{ENOTDIR, "NotADirectory"},
                                            {EEXIST, "NotADirectory"}};
  size_t n = strlen(path);
  char *prefix = emela_fs_alloc(n + 1);
  for (size_t i = 0; i <= n; i++) {
    if (i < n && (path[i] != '/' || i == 0)) continue;
    memcpy(prefix, path, i);
    prefix[i] = '\0';
    if (i > 0 && path[i - 1] == '/') continue;
    if (mkdir(prefix, 0777) != 0) {
      int e = errno;
      /* Already a directory (macOS gives EISDIR for "/"). */
      if (emela_fs_dir(prefix)) continue;
      free(prefix);
      emela_fs_complete(task, emela_fs_classify(e, "mkdir", path, codes, 2));
      return;
    }
  }
  free(prefix);
  if (!emela_fs_dir(path)) {
    emela_fs_complete(task, emela_fs_failed("NotADirectory", emela_fs_strdup("")));
    return;
  }
  emela_fs_complete(task, emela_fs_ok());
}

/* Removes a file or an empty directory. A symbolic link is removed itself, not its target. */
void emela_fs_remove(const char *path, em_task *task) {
  static const emela_fs_code_map codes[] = {{ENOTEMPTY, "DirectoryNotEmpty"},
                                            {EEXIST, "DirectoryNotEmpty"}};
  struct stat st;
  if (lstat(path, &st) != 0) {
    emela_fs_complete(task, emela_fs_classify(errno, "lstat", path, codes, 2));
    return;
  }
  bool is_dir = S_ISDIR(st.st_mode);
  if ((is_dir ? rmdir(path) : unlink(path)) != 0) {
    emela_fs_complete(task, emela_fs_classify(errno, is_dir ? "rmdir" : "unlink", path, codes, 2));
    return;
  }
  emela_fs_complete(task, emela_fs_ok());
}

char *emela_fs_outcome_error(void *o) {
  return emela_fs_strdup(((emela_fs_outcome *)o)->error);
}

char *emela_fs_outcome_message(void *o) {
  const char *m = ((emela_fs_outcome *)o)->message;
  return emela_fs_strdup(m != NULL ? m : "");
}

char *emela_fs_outcome_text(void *o) {
  const char *t = ((emela_fs_outcome *)o)->text;
  return emela_fs_strdup(t != NULL ? t : "");
}

int32_t emela_fs_outcome_name_count(void *o) { return (int32_t)((emela_fs_outcome *)o)->count; }

char *emela_fs_outcome_name_at(void *o, int32_t i) {
  emela_fs_outcome *out = o;
  if (i < 0 || (size_t)i >= out->count) return emela_fs_strdup("");
  return emela_fs_strdup(out->names[i]);
}

/* Emela does not free host values (spec 12.3): the handler releases each outcome. */
void emela_fs_outcome_release(void *o) {
  emela_fs_outcome *out = o;
  free(out->message);
  free(out->text);
  for (size_t i = 0; i < out->count; i++) free(out->names[i]);
  free(out->names);
  free(out);
}
