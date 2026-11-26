# Third-party notices

Pulse is copyright (c) 2026 n8builds.dev and is released under the MIT licence (see the repository).
It includes the software and fonts below.

## Slint

Pulse's interface is built with [Slint](https://slint.dev).
Pulse uses Slint under the Slint Royalty-free Desktop, Mobile, and Web Applications License v2.0.
The licence text is at <https://slint.dev/terms-and-conditions#royalty-free> (Slint's own sources also carry it as `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`).
Its attribution condition is met by the "About Slint" notice (the AboutSlint widget) in Pulse's Settings.
The Slint crates are dual or triple licensed (GPL-3.0-only, Slint Royalty-free, or Slint commercial); Pulse does not use the GPL option.

## Skia

Pulse draws with Skia, through the `skia-safe` and `skia-bindings` crates.
Skia is licensed under the BSD 3-Clause licence.

```text
Copyright (c) 2011 Google Inc. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

  * Redistributions of source code must retain the above copyright
    notice, this list of conditions and the following disclaimer.

  * Redistributions in binary form must reproduce the above copyright
    notice, this list of conditions and the following disclaimer in
    the documentation and/or other materials provided with the
    distribution.

  * Neither the name of the copyright holder nor the names of its
    contributors may be used to endorse or promote products derived
    from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Fonts

Pulse embeds these fonts under the SIL Open Font License, Version 1.1.
The full licence texts are in `apps/native/fonts/` and are installed beside the app as `OFL-Inter.txt` and `OFL-SpaceGrotesk.txt`.

- Inter (weights 400, 500, 600): Copyright 2020 The Inter Project Authors (<https://github.com/rsms/inter>).
- Space Grotesk (weight 700): Copyright 2020 The Space Grotesk Project Authors (<https://github.com/floriankarsten/space-grotesk>).

## Product marks

The small logos beside Linux programs in "Top apps" (Claude, Next.js, Node.js, Rust, Chrome and others) are paths from [Simple Icons](https://github.com/simple-icons/simple-icons) (version 16.38.1), embedded in the app from `apps/native/assets/marks/`.
Simple Icons publishes its SVG files under CC0 1.0 Universal (public domain dedication); the licence text is in `apps/native/assets/marks/LICENSE-simple-icons.md`.
The logos themselves remain trademarks of their owners.
Pulse is not affiliated with or endorsed by any of them, and shows a mark only to say which program a row is.

Simple Icons lists a separate licence for the artwork of three of these marks, and that licence applies to them:

- Git logo: CC BY 3.0 (the Git logo by Jason Long, <https://git-scm.com/downloads/logos>).
- Rust logo: CC BY-SA 4.0 (the Rust Foundation, <https://github.com/rust-lang/rust-artwork>).
- Deno logo: MIT (the Deno authors, <https://github.com/denoland/deno>).

The marks are drawn unchanged, only scaled and coloured white or black on a tile.
