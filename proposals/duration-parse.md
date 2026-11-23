---
title: Reading a Duration from text, and Encode, Decode and Shaped for Duration
authors: [somnicattus]
status: accepted
issue:
touches: [api.prelude, lib.layers, api.std.duration, api.std.dynamic, api.std.shape, open.list]
requires: []
related: [instant-parse]
pr: 737
decided: 2026-11-24
---

# Reading a Duration from text, and Encode, Decode and Shaped for Duration

## Summary

Add `Duration.parse(s: String) -> Duration fails ParseError`. It reads a length of time in two forms: numbers with units (`1h30m`, `1.5s`, `2h 30min`, `30 seconds`), and the time part of an ISO 8601 duration (`PT1H30M`, `-PT1.5S`). The units are the hour and shorter ones; days and weeks are not read. Add `Duration.to_iso8601`, which writes the ISO 8601 form. `to_string` keeps its form (`1500ms`), which answers `[open.list]` 48. Duration also gets Encode, Decode and Shaped: Encode writes the text of `to_string`, Decode reads text with `parse`, and the shape is a String with no note. The range (an Int64 of milliseconds) and the unit of Duration do not change.

## Motivation

There is no way in std to make a Duration from text. A Duration comes only from `milliseconds`, `seconds`, `minutes` and `hours`, and from `Instant.since` (`[api.std.duration]`); `to_string` writes `1500ms`, but nothing reads it back.

- A timeout in a configuration file or an environment variable (`"30s"`, `"PT30S"`) cannot become a Duration without the user writing the reading of units by hand.
- Duration has no Encode or Decode, so a record with a Duration field cannot derive them (E0326, checked with `emela check` of 0.28.1). Duration is an opaque type of std, so a user cannot write those impls either (E0331, `[trait.impl]`); the only way is to wrap it in a type of one's own.

```emel
import Std.Dynamic.{Decode, Encode}
import Std.Duration.{Duration}

type Job(name: String, timeout: Duration)
  derive Encode, Decode # E0326 today: Duration implements neither
```

Unlike a point in time, a length of time has no single standard text form (Prior art). People write numbers with units in configuration (`30s`, `1h30m`, `2h 30min`); data exchanged between programs uses the ISO 8601 duration (`PT1H30M`); other systems write a clock form (`01:30:00`) or a bare number whose unit depends on the writer. This proposal reads the first two. A clock form is easily confused with a time of day, and a bare number does not say its unit (Alternatives).

`[open.list]` 48 asks whether `to_string` should keep the form `1500ms` or use forms such as `1.5s` or `1h2m3s`. Adding `parse` commits to `parse(to_string(d)) == d`, so this proposal decides it: `parse` reads `1500ms`, so the form can stay.

## Proposal

```emel
import Std.Duration.{Duration}

@test
fn reads_the_forms() {
  d = Duration.milliseconds(5400000)
  assert Duration.parse("1h30m") == d
  assert Duration.parse("1.5h") == d
  assert Duration.parse("1h 30min") == d
  assert Duration.parse("PT1H30M") == d
  assert Duration.parse(d.to_string()) == d
  assert d.to_string() == "5400000ms"
  assert d.to_iso8601() == "PT1H30M"
}
```

### Meaning

- `parse` reads a length of time and returns it as a Duration. Text that does not have one of the forms, and a length outside the range of Duration, fail with `ParseError(input: s)`. The name and the error follow `[lib.api-rules]`: `parse` is a conversion from a string that can fail, and malformed input fails with the Prelude's ParseError.
- The forms are given by the grammar in `[api.std.duration]`. In short:
  - Numbers with units (`unit_form`): components from the longest unit to the shortest, each unit at most once. The units are the hour, the minute, the second, the millisecond, the microsecond and the nanosecond, each with a short name (`h`, `m`, `s`, `ms`, `us`, `ns`), an abbreviation (`hr`, `min`, `sec`, `msec`, `usec`, `nsec`) and the English word in the singular and the plural (`hour`, `hours`). The units are lowercase. One space may come between a number and its unit and between two components. `0` alone is the length 0.
  - The time part of an ISO 8601 duration (`iso_form`): `PT` followed by hours, minutes and seconds (`PT1H30M`, `PT1.5S`), in either case and without whitespace.
  - A sign comes only at the start and applies to the whole length (`-1h30m`, `-PT1H30M`). A component has no sign of its own.
  - Only the last component may have a fraction. The fraction of an hour or a minute has at most two digits; the fraction of a second or a shorter unit may have any number of digits. The digits below the millisecond are dropped, which truncates toward 0.
  - Days, weeks, months and years are not read. A day of the calendar is not a fixed length (daylight saving time, and a leap hour that may replace leap seconds), and Emela has no calendar.
  - A comma is reserved for the decimal sign, and the plurals of the abbreviations (`hrs`, `mins`) are reserved as names of their units: they are not read yet and are not given another meaning.
  - Text of more than 256 code points fails before it is read, as in `Instant.parse`.
- `to_string` does not change: the number of milliseconds followed by `ms` (`0ms`, `-1500ms`, `5400000ms`). `to_iso8601` writes the ISO 8601 form with a leading sign and without days (`PT48H`, `-PT0.5S`, `PT0S`).
- For every Duration `d`, `parse(to_string(d)) == d` and `parse(to_iso8601(d)) == d`.
- Encode writes `Text(Duration.to_string(d))`. Decode reads Text with `Duration.parse`, so it reads both forms. A Number is not read: its unit is not in the value. Shaped is the shape of String inside `Named("Duration", ...)`, with no `format` note, because the text that Encode writes is not an ISO 8601 duration.
- A user who wants the ISO 8601 form in Dynamic, or the note `format: "duration"`, wraps Duration in a type of their own (`type IsoDuration(value: Duration)`) and writes its Encode, Decode and Shaped with `to_iso8601` and `parse`. The type is in the user's module, so E0331 does not apply.

### Typing rules

No new typing rules.

## Diagnostics

- [x] None.

## Compatibility

No program that compiles now changes meaning.

- `Std.Duration` gains the pub fns `parse` and `to_iso8601`. A method call `s.parse()` looks up the fns of the module that defines String (`[type.rules]`), so existing calls do not become ambiguous.
- Duration gains Encode, Decode and Shaped. Users could not write these impls (E0331), so no program becomes E0330.
- The output of `to_string` and Show does not change.
- Each restriction in Alternatives (no comma, components in one order, no repeated unit, no uppercase units, no plurals of abbreviations, at most one space, fractions on the last component only) and the limit of 256 code points can be lifted later without changing the result of any text that `parse` reads now.

## Implementation

- emela-core (std sources): write `parse` and `to_iso8601` in Emela in `std/duration.emel`. Add the impls for Duration to `std/dynamic.emel` and `std/shape.emel`, where the impls for Bytes, Array, Map and Set are (`[trait.impl]` allows either module; users see no difference). The doc comment of `to_string` refers to `[api.std.duration]`.
- No change to the compiler.
- `parse` checks the length first, then reads the code points of `s` once from the start (`String.code_points`), not with a slice per character, which would be O(n²). No Float is used.
- Every component before the last is a whole number, and no component has a sign, so the truncated length is the sum of the whole milliseconds of every component and of the fraction of the last one. Only the leading digits of that fraction can change the result: two for an hour or a minute (0.01 hour is 36,000 milliseconds and 0.01 minute is 600), three for a second, none for a millisecond; a microsecond or a nanosecond adds the whole number divided by 10³ or 10⁶, truncated, and its fraction is dropped. The rest of the fraction is checked to be digits and skipped.
- The magnitude of the shortest Duration, 9,223,372,036,854,775,808 milliseconds, is one more than the largest Int64. `parse` accumulates the length so that this value can be reached when the sign is `-` (for example, accumulating the negated magnitude), and fails as soon as the magnitude passes the bound of its sign; no intermediate value wraps around. `to_iso8601` likewise takes the hours, minutes and seconds of the shortest Duration without negating it first.
- Tests: `parse(to_string(d)) == d` and `parse(to_iso8601(d)) == d` at 0, ±1 ms, ±999 ms, ±1 s, 1h30m, 48 hours, and both ends of Int64; the text one past each end fails. The output of `to_iso8601` at those values. Each example and counterexample in `[api.std.duration]`. Every name of every unit, and both micro signs. Text of exactly 256 code points that is read (a long fraction of a second), and text of 257 code points that fails. Fractions of 20 digits or more. Decode of Text in both forms, of a Number (`expected: "text"`) and of malformed Text (`expected: "duration"`). The same results on the three targets.

## Alternatives

- **Change `to_string` to numbers with several units** (`1h30m`, `1.5s`). `parse` reads `1500ms`, so the form need not change for the round trip, and changing it would change the output of programs that use `to_string` and Show. It would also need rules on leaving out components of 0 at the start and in the middle (`1h0m0s`), on lengths under a second, and on which `µ` to write.
- **Encode as `to_iso8601`, with the note `format: "duration"` on the shape.** The grammar that the note refers to (RFC 3339 Appendix A) has neither fractions nor signs, so every length that is not a whole number of seconds (`PT1.5S`) and every negative length (`-PT1S`) would not fit the note. Encoding the text of `to_string`, as Instant does, keeps one written form. A user who needs the ISO 8601 form wraps Duration in a type of their own.
- **A way in std to choose the written form** (a function that takes the form as a value, a parameter with a default, a handler member). std writes at most two forms, and a function for each is enough. A user who wants to choose writes a type for the forms. Emela has no parameters with defaults (`[fn.args]`: a wrong number of arguments is E0305), and a handler member would make a pure conversion need `uses` and a `with`.
- **No limit on the length of `s`.** No writer of a length of time comes near 256 code points: the longest text of `to_string` has 22, of `to_iso8601` 27, and hours to milliseconds written with full names 58. The limit bounds the work of `parse` and the digits of each number whatever text it is given. It counts code points, as `Shape.max_length` does, and is the same as the limit of `Instant.parse`. It can be raised later.
- **Read a clock form** (`01:30:00`, `[-][d.]hh:mm:ss[.fffffff]`, `3 days 04:05:06`). It is hard to tell apart from the time of day of an Instant. The writers also disagree on the sign: in `-1.02:03:04` the sign applies to the whole length, while other writers put a sign on each field.
- **Read a sign on each component of `iso_form`** (`PT-1.3S`, `PT-4H-5M-6S`). `PT1H-30M` looks like two parts joined by a hyphen, so `-` is easy to misread as a separator. ISO 8601 itself has no sign on a component; a leading sign (`-P1Y1D`) is an extension of ISO 8601-2. Negative lengths written with a sign on each component cannot be read.
- **Only the short names of units** (`h`, `m`, `s`, `ms`, `us`, `ns`). `2h 30min` and `30 seconds`, which people write in configuration, could not be read.
- **Read uppercase units in `unit_form`** (`1H30M`, `30S`). No writer of them was found, and some writers use `M` for a month, so reading `1M` as a minute would misread their text.
- **Match the singular and the plural to the number** (only `1 second` and `2 seconds`). It needs a rule for fractions (`1.5 hour`) and for 0, and brings the grammar of English into `parse`.
- **Any number of spaces.** One space, as in `Instant.parse`. It can be relaxed later.
- **A fraction on any component** (`1.5h30m`). No writer of it was found, and ISO 8601 allows a fraction only on the lowest-order component. It can be relaxed later.
- **Any number of digits in the fraction of an hour or a minute.** The limit of two digits is a guard for the writer, not a rule needed for exactness: no writer of three or more digits was found, and such a length reads better in a smaller unit. It can be relaxed later.
- **At most three digits in the fraction of a second or a shorter unit.** Lengths written to the microsecond or the nanosecond (`3.000001s`, `1h15m30.918273645s`, `140.884ms`) could not be read. `Instant.parse` also reads any number of digits.
- **A comma as the decimal sign** (`PT1,5S`). ISO 8601 allows it, but no writer of it was found. It can be added later; the comma is reserved for it.
- **Round to the nearest millisecond.** `0.9995s` would become 1000 milliseconds, adding a digit that is not written. Dropping the digits keeps the components as written, and `Instant.parse` does not round either.
- **Truncate toward negative infinity**, the direction of `Instant.parse`. An Instant has no meaningful 0 (the Unix epoch is a convention), so it is truncated toward the past. The 0 of a Duration is no length at all, and dropping digits of a negative length makes its magnitude smaller, which is truncation toward 0 (`-0.0005s` is 0). `to_seconds` also truncates toward 0.
- **Components in any order** (`30s1h`). Writers write the longest unit first. It can be added later without changing the result of any text read now.
- **Sum a repeated unit** (`300ms20s 5day`). A component has no sign, so the repetitions can be written as one component, and no writer repeats a unit.
- **Read a number without a unit** (`1500`). Writers differ on its unit (nanoseconds, milliseconds, seconds), and the value does not say which. `0` alone is read, since its length does not depend on the unit.
- **Decode a Number.** The same reason as a number without a unit. The Decode of Instant does not read a Number either.
- **Read days and weeks as 24 hours and 7 days.** A day of the calendar is not 24 hours (`[api.std.duration]`), and a fixed day could be mistaken for one. Lengths written with days (`P3DT4H5M6S`) cannot be read; lengths written in hours without days (`PT48H`, `48h0m0s`) are read.

## Prior art

- Numbers with units:
  - Go `time.Duration`: `String` writes `72h3m0.5s`, leaving out leading units of 0 and keeping the ones in the middle (`10h0m0s`), with a smaller unit under a second (`300ms`) and `0s` for 0. `ParseDuration` reads "a possibly signed sequence of decimal numbers, each with optional fraction and a unit suffix", with the units `ns`, `us` (`µs`), `ms`, `s`, `m` and `h`; its documentation states no rule on order or repetition. encoding/json writes a Duration as a number of nanoseconds.
  - Prometheus: integers with the units `y` (365 days), `w`, `d` (24 hours), `h`, `m`, `s` and `ms`, from the longest unit, each at most once, without fractions; `0` alone is allowed (`1h30m`, `54s321ms`).
  - systemd: abbreviations and English words (`usec`, `us`, `μs`, `msec`, `seconds`, `sec`, `minutes`, `min`, `hours`, `hr`, `days`, `weeks`, `months` of 30.44 days, `years` of 365.25 days), with optional spaces; all components are summed, repeated or not (`300ms20s 5day`). It writes `2h 30min`.
  - Kotlin `kotlin.time.Duration`: `toString` writes `1h 30m` and `1h 0m 30.340s`, with a fraction on the last component only, and `-(1h 30m)` for a negative length of several components; `parse` reads that form and ISO 8601, and does not read `1 hour 30 minutes`.
  - CSS Values 4: the time units `s` and `ms` (7.2); "unit identifiers are ASCII case-insensitive" (5.4).
  - protobuf `google.protobuf.Duration` in ProtoJSON: seconds with the suffix `s`; the output always has 0, 3, 6 or 9 digits of fraction (`1.500s`), and the input may have any number of digits that fit in nanoseconds. The range is ±315,576,000,000 seconds. The page notes that it is not the RFC 3339 duration format.
- The ISO 8601 duration:
  - RFC 3339 Appendix A gives an ABNF of the ISO 8601 duration, "informational only", with integers and the designators `Y M W D` and, after `T`, `H M S`, without fractions or signs. The `format: "duration"` of JSON Schema refers to it.
  - Java `java.time.Duration`: `toString` writes `PTnHnMnS` without days (`PT48H`) and writes a negative length with a sign on each component (`PT-1.3S`); `parse` reads a leading sign, a sign on each component, days, a fraction of seconds only, `.` or `,` as the decimal sign, and either case.
  - TC39 Temporal: a leading sign (`-P1Y1D`), and `PT0S` for 0.
  - pydantic: `timedelta` is written in JSON as the ISO 8601 duration by default, or as a number of seconds; from 2.12 `ser_json_temporal` also offers milliseconds.
- Clock forms:
  - .NET `TimeSpan`, the format `"c"`: `[-][d.]hh:mm:ss[.fffffff]`, with the sign on the whole length (`3.17:25:30.5000000`, `-10:27:22`). System.Text.Json writes a TimeSpan as a string (`"2.00:00:01"`) since .NET 6.
  - Python `str(timedelta)`: `1 day, 2:03:04.500000`; a negative length has a negative day and a positive time (`-1 day, 23:59:58.500000`).
  - PostgreSQL `interval`: `3 days 04:05:06` by default, with a sign on each field (`-1 year -2 mons +3 days -04:05:06`); with `IntervalStyle` `iso_8601`, `P3DT4H5M6S` and `P-1Y-2M3DT-4H-5M-6S`.

## Open questions

## Decision

Adopted. A Duration could be written as text with `to_string` but not read back, and a record with a Duration field could not derive Encode or Decode; since Duration is an opaque type of std, the only way around it was a wrapping type.

- `parse` reads two forms: numbers with units, which people write in configuration, and the time part of an ISO 8601 duration, which programs exchange. It reads the output of `to_string`, so `to_string` keeps the form `1500ms` and `[open.list]` 48 is decided without changing any output.
- Units stop at the hour. A day of the calendar is not a fixed length, and Emela has no calendar, so reading a day as 24 hours would let a length be mistaken for a span of the calendar.
- A sign applies to the whole length, and only the last component has a fraction. Lengths below the millisecond are truncated toward 0, and a length outside the range fails instead of wrapping around.
- Each restriction (no comma, components in one order, no repeated unit, lowercase units, no plurals of abbreviations, one space, the limit of 256 code points) can be lifted later without changing the result of any text read now. The limit, the reserved comma and the single space match `Instant.parse`.
- Duration is encoded as the text of `to_string`, as Instant is, and its shape has no `format` note, because that text is not an ISO 8601 duration. A user who needs the ISO 8601 form in Dynamic wraps Duration in a type of their own.
