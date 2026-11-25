---
title: Reading an offset with seconds in Instant.parse
authors: [somnicattus]
status: accepted
issue:
touches: [api.std.instant]
requires: []
related: [instant-parse]
pr: 868
decided: 2026-11-25
---

# Reading an offset with seconds in Instant.parse

## Summary

`Instant.parse` also reads an offset from UTC that has seconds, written `±HH:MM:SS` or `±HHMMSS`. Within one offset, either every field after the hour follows a colon or none does, and the second of the offset is 00 to 59, with no fraction. Every text that `parse` read before the change reads as the same Instant after the change.

## Motivation

`Instant.parse` (`[api.std.instant]`) reads the offsets `Z`, `z`, `±HH:MM`, `±HHMM` and `±HH`, and fails on an offset that has seconds. The proposal `instant-parse` (#730) chose not to read one (its Alternatives), for two reasons: PostgreSQL writes an offset with seconds only for times in the period of local mean time, and ISO 8601 has no offset with seconds.

The time zone database (tzdata) starts most zones with their local mean time (LMT), the mean solar time at the location that names the zone. In tzdata 2026e, the offset of the earliest period is not a whole number of minutes in 363 of the 447 zones. `Asia/Tokyo` has the offset `+09:18:59` until 1887-12-31 15:00 UTC. `Europe/Dublin` has `-00:25:21` (Dublin Mean Time) until 1916-05-21, and `+00:34:39` (Dublin Mean Time with one hour of summer time) from then until 1916-10-01. The writers in the table below write the seconds of the offset when they format a time of one of these periods in the zone of the period. Checked with PostgreSQL 16, OpenJDK 21 (`java.time`), Python 3.13 and Go, for the time 1880-01-01 00:00:00 in `Asia/Tokyo` and 1900-01-01 00:00:00 in `Europe/Dublin`:

| Writer | Text written |
|---|---|
| Output of `timestamptz` in PostgreSQL (`DateStyle` ISO) | `1880-01-01 00:00:00+09:18:59` |
| `to_json` of a `timestamptz` in PostgreSQL | `1880-01-01T00:00:00+09:18:59` |
| `toString()` of `OffsetDateTime` in Java | `1900-01-01T00:00-00:25:21` |
| `ISO_OFFSET_DATE_TIME` of `DateTimeFormatter` in Java | `1880-01-01T00:00:00+09:18:59` |
| The patterns `xxxx` and `XXXX` of `DateTimeFormatter` in Java | `+091859` |
| `isoformat()` of a `datetime` of Python with a `ZoneInfo` | `1900-01-01T00:00:00-00:25:21` |
| The directive `%:z` of `strftime` in Python | `+09:18:59` |
| The directive `%z` of `strftime` in Python, in `"%Y-%m-%dT%H:%M:%S%z"` | `1880-01-01T00:00:00+091859` |
| The layouts `-07:00:00` and `-070000` of `Time.Format` in Go | `+09:18:59` and `+091859` |

Readers: PostgreSQL reads `+09:18:59` as input to `timestamptz`, and `OffsetDateTime.parse` of Java reads `-00:25:21`; neither reads `+091859`. `datetime.fromisoformat` of Python, and `datetime.strptime` of Python with `%z`, read both `-00:25:21` and `-002521`. `datetime.fromisoformat` does not read an offset that has a colon after the hour and none after the minute, or the reverse (`+0918:59`, `+09:1859`).

Other formats write the hours and the minutes of the offset only, and drop its seconds: for 1880-01-01 00:00:00 in `Asia/Tokyo`, the layout `time.RFC3339` of Go, `Time#iso8601` of Ruby and `DATE_ATOM` of PHP write `1880-01-01T00:00:00+09:18`, and `Time#to_s` of Ruby writes `1880-01-01 00:00:00 +0918`. `parse` reads these texts already; the Instant it gives is 59 seconds later than the time that was formatted, and `parse` cannot recover the dropped seconds. This proposal does not change how these texts are read.

Text with an offset that has seconds is rare: the writers in the table write one only for a time in a period whose offset is not a whole number of minutes, and in tzdata 2026e the last of those periods ends in 1972 (`Africa/Monrovia`, `-00:44:30`). `parse` fails on each text of the table, and a record with an Instant field fails to decode when the field holds one of those texts. To read that field, a user wraps Instant in a type of their own whose Decode removes the seconds from the offset, reads the remaining text with `parse`, and moves the result earlier by the removed seconds for an offset with `+` and later for an offset with `-`; the user writes the grammar of the offset again, outside std.

```emel
import Std.Dynamic.{Decode, DecodeError}
import Std.Instant.{Instant}
import Std.Json
import Std.Json.{SyntaxError}

type Reading(id: Int, at: Instant) derive Decode

fn read() -> Reading fails DecodeError | SyntaxError {
  # A row of `to_json` of PostgreSQL, with `timezone` set to Asia/Tokyo.
  Json.decode("{\"id\": 1, \"at\": \"1880-01-01T00:00:00+09:18:59\"}")
  # Fails with emela 0.28.0: DecodeError(expected: "date-time", ...)
}
```

The Compatibility of `instant-parse` states that each restriction in its Alternatives, the one on offsets with seconds included, can be lifted later without changing the result of any text that `parse` reads.

## Proposal

```emel
import Std.Instant.{Instant}

@test
fn reads_an_offset_with_seconds() {
  # `to_json` of PostgreSQL, with `timezone` set to Asia/Tokyo.
  assert Instant.parse("1880-01-01T00:00:00+09:18:59") == Instant.parse("1879-12-31T14:41:01Z")
  # `toString()` of `OffsetDateTime` in Java leaves out the seconds of the time when they are 00.
  assert Instant.parse("1900-01-01T00:00-00:25:21") == Instant.parse("1900-01-01T00:25:21Z")
  # `%z` of `strftime` in Python writes the offset without colons.
  assert Instant.parse("1880-01-01T00:00:00+091859") == Instant.parse("1879-12-31T14:41:01Z")
}
```

### Syntax

The rule `offset` of the grammar of `parse` in `[api.std.instant]` becomes:

```ebnf
offset    = "Z" | "z" | [ " " ] , sign , digit2 , [ ( ":" , digit2 , [ ":" , digit2 ] ) | ( digit2 , [ digit2 ] ) ] ;
```

- An offset has a second only if it has a minute, and the second comes after the minute. When a colon separates the hour and the minute, a colon also separates the minute and the second (`+09:18:59`); when no colon separates the hour and the minute, none separates the minute and the second (`+091859`). `+0918:59` and `+09:1859` do not match the grammar.
- The offsets that the grammar matched before (`Z`, `z`, `±HH:MM`, `±HHMM`, `±HH`, each numeric one with or without one space before it) match the new rule as well.

### Meaning

In the paragraph of `[api.std.instant]` that gives the ranges of the fields, the sentence on the ranges of the offset and the sentence on the offset 0 are replaced by the text below. The other sentences of the paragraph, including the one on `-00:00`, do not change.

> The hour of the offset is 00 to 23, its minute is 00 to 59, and its second is 00 to 59. An offset written with the hour only (`+09`) has the minute 00 and the second 00, and an offset written with the hour and the minute (`+09:00`, `+0900`) has the second 00. `Z`, `z`, and a numeric offset whose hour, minute and second are all 00 (`+00:00`, `-0000`, `+00`, `+00:00:00`, `-000000`) are the offset 0.

- The rule that gives the point in time does not change: the point in time is the date and time minus the offset. If the point is outside the range of Instant, `parse` fails, as before.
- `to_string` does not change. It writes the offset `Z` only.
- The Decode of Instant reads Text with `parse` (`[api.std.dynamic]`), so the Decode reads offsets with seconds too. The Encode of Instant and the note `format: "date-time"` of the Shaped impl of Instant (`[api.std.shape]`) do not change.

### Typing rules

No new typing rules.

## Diagnostics

- [x] None.

## Compatibility

Every program that compiles before the change compiles after the change, with the same types. Every text that `parse` read before the change reads as the same Instant after the change.

- Text with an offset `±HH:MM:SS` or `±HHMMSS` made `parse` fail with `ParseError`, and is read after the change. A program that relies on that failure, for example to reject text with an offset that has seconds, gets an Instant instead.
- The output of `to_string` and of the Encode of Instant does not change.

## Implementation

- emela-core (std sources): in `std/instant.emel`, `numeric_offset` returns the offset in seconds east of UTC instead of minutes. When text remains after the minute of the offset, `numeric_offset` reads that text as `:` and two digits if a colon came after the hour, and as two digits if no colon came after the hour; any other remaining text fails. `parse` subtracts the offset, in milliseconds, from the milliseconds since the start of the day; the difference then lies between -86399000 and 172798999, and the comment that states those bounds before `local` in `parse` changes with them. The range of Instant is checked on the result, as before.
- No change to the compiler.
- A prototype of the change to `numeric_offset` and `parse` passed the tests of `tests/laws` on the JS, native and WASM targets, except `an_offset_with_seconds_is_not_read`, which this proposal replaces. The prototype also read the first and the last point of Instant written with `+23:59:59` and `-235959`, and failed on the points 1 ms outside them and on each of the failing offsets listed under Tests.
- Tests, in `tests/laws/test/instant_parse_test.emel`:
  - Each text of the table in Motivation, completed to a date and time where the table shows the offset only, reads as the Instant it denotes.
  - The first and the last point of Instant, written with each of the offsets `+23:59:59`, `-23:59:59`, `+235959` and `-235959`, are read; the points 1 ms outside them fail.
  - `+00:00:00`, `-00:00:00`, `+000000` and `-000000` are the offset 0.
  - A time without seconds followed by an offset with seconds (`1900-01-01T00:00-00:25:21`, `1900-01-01T00:00-002521`) is read.
  - Each of these offsets, after a date and time, fails: `+09:18:60`, `+091860`, `+0918:59`, `+09:1859`, `+09:18:5`, `+09185`, `+0918590`, `+09:18:59.5`, `+091859.5`, `+09:18:59,5`, `+09:18:59:00`.
  - The test `an_offset_with_seconds_is_not_read`, which checks that `+09:00:00`, `+090000` and `+09:18:59` fail, is replaced by the tests in this list. The offsets in the test `a_malformed_offset_is_not_read` still fail.
  - The same results on the three targets.

## Alternatives

- **Keep failing on an offset with seconds**, as `instant-parse` chose. The texts of the table in Motivation stay unreadable, and so does a record whose Instant field holds one of them.
- **Read `±HH:MM:SS` but not `±HHMMSS`.** ISO 8601 has no offset with seconds, with colons or without them, and PostgreSQL and `OffsetDateTime.parse` of Java do not read `±HHMMSS` (PostgreSQL fails with "time zone displacement out of range"). Not chosen: `%z`, the directive of `strftime` in Python for the offset, writes `±HHMMSS`, and the grammar already reads `±HHMM` as well as `±HH:MM`. A rule that either every field of the offset after the hour follows a colon or none does is simpler than a rule that allows the seconds after a colon only. The six digits of `±HHMMSS` are not ambiguous: nothing may follow the offset, so the number of digits after the sign (2, 4 or 6) decides which fields are written.
- **Read an offset with a colon after the hour and none after the minute, or the reverse** (`+0918:59`, `+09:1859`). None of the writers in Motivation writes them, and `datetime.fromisoformat` of Python does not read them.
- **Read a fraction of the second of the offset** (`+00:00:00.001500`). `isoformat()` and `%z` of Python write a fraction for a `datetime.timezone` built with an offset that has microseconds. tzdata holds offsets in whole seconds, so no zone of tzdata has an offset with a fraction of a second, and PostgreSQL does not read an offset with a fraction. Reading a fraction would also need a rule for the digits of the fraction after the millisecond. It can be added later without changing the result of any text that this proposal reads.
- **Read an offset with seconds, and drop its seconds.** The Instant would be up to 59 seconds away from the time that was written, and the user would not be told.

## Prior art

- Offsets from UTC with seconds: the field `STDOFF` of the Zone lines of tzdata (read by zic); `ZoneOffset` of `java.time`, written `±HH:MM:SS` with the seconds left out when they are 00; `datetime.timezone` of Python 3.7 and later, written `+HH:MM:SS[.ffffff]` by `isoformat` and `+HHMMSS[.ffffff]` by `%z`; the layouts `-07:00:00` and `-070000` of Go; the output and the input of `timestamptz` in PostgreSQL.
- ISO 8601-1:2019 and RFC 3339 give offsets of hours and minutes only: `±HH:MM` in both, and also `±HHMM` and `±HH` in ISO 8601.

## Open questions

## Decision

Real systems write offsets with seconds for times in the period of local mean time (PostgreSQL, Java, Python, Go), and a record holding such an Instant could not be decoded at all. Every text that `parse` read before reads as the same Instant, and `to_string`, Encode and Shaped do not change. Reading `±HHMMSS` keeps the offset forms symmetric with `±HHMM`, and mixing colons within one offset stays rejected.
