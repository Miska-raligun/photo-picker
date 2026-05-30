# Third-party licenses

photo-pick is dual-licensed MIT OR Apache-2.0 (see the top-level Cargo.toml).
It links against the following third-party crates whose licenses are stricter
than the project's own; the corresponding texts are stored here.

| Crate | License | Notes |
|---|---|---|
| [`rawler`](https://crates.io/crates/rawler) | [LGPL-2.1](./LGPL-2.1.txt) | RAW preview extraction. We use the public API only; our code is not derived from rawler's internals. |

## What LGPL-2.1 means for redistribution

LGPL-2.1 permits commercial redistribution but Section 6 requires that, when
the library is statically linked into a larger work (as we do via Rust's
default linking), the distributor provides one of:

1. **The complete corresponding source** of the LGPL component (rawler's
   source is on [GitHub](https://github.com/dnglab/dnglab); pinning the
   version in `Cargo.toml` is sufficient), **and**
2. A means for the user to relink the binary against a modified version of
   rawler (typically: making the object files available, or distributing as
   open source).

Our release tarballs include this directory so the LGPL-2.1 text travels
with every binary. If photo-pick ever moves to a closed-source commercial
distribution, the simplest path is to either continue providing object files
for the LGPL'd portion or switch to a more permissive RAW decoder.
