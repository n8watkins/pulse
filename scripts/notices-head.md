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

The small logos beside Linux programs in "Top apps" (Claude, Next.js, Node.js, Chrome, Rust, OpenAI, Visual Studio Code and others) are paths from [Simple Icons](https://github.com/simple-icons/simple-icons) (version 16.38.1), embedded in the app from `apps/native/assets/marks/`.
Simple Icons publishes its SVG files under CC0 1.0 Universal (public domain dedication); the licence text is in `apps/native/assets/marks/LICENSE-simple-icons.md`.
The logos themselves remain trademarks of their owners.
Pulse is not affiliated with or endorsed by any of them, and shows a mark only to say which program a row is.

Two marks carry a licence of their own in the Simple Icons data, and it applies to them:

- Git logo by Jason Long, licensed under [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/).
- Deno logo, under the MIT licence of the Deno docs repository, text below.

```text
MIT License

Copyright (c) 2018-2024 the Deno authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The Rust mark carries a share-alike licence:

- Rust logo, copyright the Rust Foundation, licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) (source <https://www.rust-lang.org>, as listed by Simple Icons 16.38.1).
  The file `apps/native/assets/marks/rust.svg` is under that licence, separate from Pulse's MIT code: the licence covers the logo file and any adaptation of it, not the rest of Pulse.
  The file is used unchanged, shown only scaled and in white on a black tile.

Two marks come from older Simple Icons releases, because current releases no longer include them.
Both were under the collection's CC0 default at that time:

- OpenAI mark (used for Codex CLI): Simple Icons 15.22.0, the last release that has it. Source <https://openai.com/brand>.
- Visual Studio Code mark: Simple Icons 12.4.0, the last release that has it. Source <https://commons.wikimedia.org/wiki/File:Visual_Studio_Code_1.35_icon.svg>.

Programs with no mark (Cursor's and Windsurf's "Editor server", Aider, opencode and others) keep the letter tile.

The marks are drawn unchanged, only scaled and coloured white or black on a tile.
Each mark with its title, source and licence as Simple Icons lists them (no licence listed means the CC0 default of the collection); the Simple Icons version is 16.38.1 unless the table says otherwise:

| Mark | Simple Icons name | Licence | Source |
| --- | --- | --- | --- |
| Claude | claude | CC0 1.0 | <https://claude.ai> |
| Google Gemini | googlegemini | CC0 1.0 | <https://gemini.google.com> |
| Cursor | cursor | CC0 1.0 | <https://cursor.com/brand> |
| Ollama | ollama | CC0 1.0 | <https://github.com/ollama/ollama/issues/2152#issuecomment-1905286922> |
| Docker | docker | CC0 1.0 | <https://www.docker.com/company/newsroom/media-resources> |
| Next.js | nextdotjs | CC0 1.0 | <https://vercel.com/design/brands#next-js> |
| Node.js | nodedotjs | CC0 1.0 | <https://nodejs.org/en/about/branding> |
| npm | npm | CC0 1.0 | <https://www.npmjs.com> |
| pnpm | pnpm | CC0 1.0 | <https://pnpm.io/logos> |
| Bun | bun | CC0 1.0 | <https://bun.sh/press-kit> |
| Deno | deno | MIT | <https://github.com/denoland/docs/blob/5dee713844c7447f80acd4093caa9d350d80bf36/static/img/logo.svg> |
| Python | python | CC0 1.0 | <https://www.python.org/community/logos/> |
| Git | git | CC BY 3.0 | <https://git-scm.com/community/logos> |
| Google Chrome | googlechrome | CC0 1.0 | <https://www.google.com/chrome> |
| Go | go | CC0 1.0 | <https://blog.golang.org/go-brand> |
| PostgreSQL | postgresql | CC0 1.0 | <https://wiki.postgresql.org/wiki/Logo> |
| Redis | redis | CC0 1.0 | <https://redis.io/brand-guidelines> |
| Vite | vite | CC0 1.0 | <https://github.com/voidzero-dev/community-design-resources/blob/55902097229cf01cf2a4ceb376f992f5cf306756/brand-assets/vite/vite-icon-color-bracketless.svg> |
| TypeScript | typescript | CC0 1.0 | <https://www.typescriptlang.org/branding> |
| esbuild | esbuild | CC0 1.0 | <https://github.com/evanw/esbuild/blob/ac542f913908d7326b65eb2e01f0559ed135a40e/images/logo.svg> |
| MongoDB | mongodb | CC0 1.0 | <https://www.mongodb.com/pressroom> |
| MySQL | mysql | CC0 1.0 | <https://www.mysql.com/about/legal/logos.html> |
| NGINX | nginx | CC0 1.0 | <https://www.nginx.com/press/> |
| Tailscale | tailscale | CC0 1.0 | <https://tailscale.com/press> |
| Rust | rust | CC BY-SA 4.0 (Rust Foundation) | <https://www.rust-lang.org> |
| OpenAI (Codex CLI) | openai (15.22.0) | CC0 1.0 | <https://openai.com/brand> |
| Visual Studio Code | visualstudiocode (12.4.0) | CC0 1.0 | <https://commons.wikimedia.org/wiki/File:Visual_Studio_Code_1.35_icon.svg> |
