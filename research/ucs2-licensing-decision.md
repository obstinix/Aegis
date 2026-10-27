# Decision note: the `ucs2` crate (MPL-2.0)

- **Status:** open, the owner's decision. Nothing has been changed: `aegis-uefi` uses `uefi-raw`, `ucs2` is not in `Cargo.lock`, and `deny.toml` is as it was.
- **Date:** 2026-10-27. Evidence: [licensing review §3.1](licensing-review.md#31-ucs2-033-source-inspection-2026-10-27).
- This is an engineering assessment, **not legal advice**.

## The question

The `uefi` crate (0.41.0, MIT OR Apache-2.0) would give `aegis-uefi` safe wrappers for UEFI protocols. It cannot be used without `ucs2` 0.3.3, which is MPL-2.0. MPL-2.0 is not on the ADR-0011 licence allowlist (MIT, Apache-2.0, BSD-2/3, ISC, Unicode-3.0, Zlib). Should Aegis allow it?

## Facts (inspected, not assumed)

- **Version:** `ucs2` 0.3.3, the latest release, published from `rust-osdev/ucs2-rs` commit `aa837529`.
- **Licence:** `license = "MPL-2.0"` in its metadata. `LICENSE` is the unmodified MPL-2.0 text.
- **Source:** two files, 353 lines of UCS-2 encoding helpers. Neither has a licence header, and no file carries the Exhibit B "Incompatible With Secondary Licenses" notice.
- **Dependencies:** only `bit_field` 0.10.3 (Apache-2.0/MIT).
- **Usage:** `uefi` 0.41 depends on it unconditionally; no feature removes it. `uefi` re-exports its `ucs2_cstr!` macro (behind `cstr16!`) and calls `ucs2::encode_with` in the text-output `fmt::Write` implementation.
- **`uefi`'s other dependencies are allowed:** `bitflags`, `cfg-if`, `log`, `ptr_meta` (MIT), `uefi-macros`, `uefi-raw`, `uguid`. `uefi` 0.41 needs Rust 1.91, below the pinned 1.97.

## Obligations if `ucs2` is allowed

MPL-2.0 is a file-level copyleft: its conditions apply to `ucs2`'s own files, not to the rest of Aegis.

| Situation | Obligation |
|---|---|
| Aegis source, with `ucs2` fetched from crates.io by Cargo | None beyond Aegis's own licence: the repository holds no `ucs2` code. |
| Distributing an `aegis-uefi` binary (Executable Form, §3.2) | Make `ucs2`'s source available and tell recipients how to get it. Naming the crate, version and repository in a `THIRD_PARTY_NOTICES.md` shipped with the binary would cover it. The binary's licence must not restrict recipients' MPL rights to `ucs2`'s source. |
| Aegis as a whole (Larger Work, §3.3) | May stay Apache-2.0, provided the `ucs2` files keep MPL-2.0 terms. |
| Modifying or vendoring `ucs2`'s files | Those files stay MPL-2.0, and the modified source must be available. Its licence notice (the `LICENSE` file; there are no headers) must be kept (§3.4). |
| Patents | MPL-2.0 has a patent grant, which terminates against anyone who brings a patent claim over the covered software (§2.3, §5.2). This is similar in effect to Apache-2.0's patent clause. |

**Attribution.** MPL-2.0 does not require crediting the authors in documentation. It requires keeping the licence notices and pointing to the source, as above. Listing `ucs2` in `THIRD_PARTY_NOTICES.md` is still the clean way to do both.

## Compatibility

- **With Apache-2.0 outbound:** compatible as a Larger Work, as above.
- **With GPL-2.0-or-later or LGPL-2.1-or-later downstream:** compatible. Without the Exhibit B notice, MPL-2.0 allows combination under those "Secondary Licenses" (§3.3).
- **With project policy:** it needs an ADR-0011 exception. A crate-scoped `cargo deny` exception keeps MPL-2.0 out of the general allowlist, so no other MPL crate slips in unnoticed:
  ```toml
  [[licenses.exceptions]]
  crate = "ucs2"
  allow = ["MPL-2.0"]
  ```

## Alternatives

1. **Keep `uefi-raw` only (today).** No licence change. The cost is hand-written wrappers for each UEFI protocol Aegis uses: three small `unsafe` blocks today, more as the UEFI side grows (memory map, file system, ACPI tables).
2. **Allow `ucs2` alone and switch to `uefi`.** Use the scoped exception above, an ADR-0011 note, and `THIRD_PARTY_NOTICES.md` before any binary is distributed.
3. **Ask upstream.** Ask `rust-osdev` whether `ucs2` could be relicensed (its siblings `uefi` and `uefi-raw` are MIT OR Apache-2.0), or whether `uefi` could drop it. It is an owner action with no guaranteed outcome, but it is cheap.
4. **Replace `ucs2` under `uefi` with a `[patch]`.** Not recommended: Aegis would maintain a fork of its API, and a clean reimplementation is more work than alternative 1.
5. **Another raw-definitions crate, such as `r-efi`** (MIT OR Apache-2.0 OR LGPL-2.1-or-later). It offers nothing over `uefi-raw`, which is already in use.

## Recommendation

**Keep alternative 1 for now. Revisit when a milestone needs enough UEFI protocols that hand-written wrappers become a real cost.** At that point choose between alternatives 1 and 2. Alternative 3 can be tried meanwhile. If alternative 2 is chosen, apply the scoped exception, record it in ADR-0011, and add `THIRD_PARTY_NOTICES.md` in the same change.

The decision, and any change to `deny.toml` or the dependency, is the owner's.
