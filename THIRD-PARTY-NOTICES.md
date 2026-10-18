# Third-party notices

rs-rize is the Rust port of the published Rize Local v1.2 source. It retains the
React renderer and replaces the Electron/Node application and service logic.
The Rust and npm inventories below describe this repository's current dependency
graphs. They do not grant a license to rs-rize's own source; no project-wide
license has been selected by its owner.

The Rize name and unchanged public favicon remain associated with their owner.
The favicon's exact source and digest are retained in [asset provenance](docs/native-asset.txt).
No proprietary Rize application bundle is included.

## Current Rust dependencies

[RUST-NOTICES.txt](docs/licenses/RUST-NOTICES.txt) records **568 locked external crates**
and reproduces their supplied upstream license, copyright, copying and notice
files, including normal, build, test and other-platform dependencies. The
[machine-readable inventory](docs/licenses/rust-dependencies.json) records exact
names, versions, SPDX declarations, package sources, notice digests and Windows
graph roles. This deliberately exceeds the subset linked into any one executable.
The Rust standard library's supplied copyright report and referenced license texts
are included; the compiler and Cargo programs are not bundled in the app.

40 crate archives supplied no standalone notice file located by the generator;
their exact declared license metadata and missing-file status are recorded without
inventing copyright text. The preserved upstream SPDX expressions can describe
alternative licenses; this document does not reinterpret those expressions.

## Current renderer dependencies

[NPM-NOTICES.txt](docs/licenses/NPM-NOTICES.txt) contains the actual installed
production package license files. The [production inventory](docs/licenses/production-dependencies.json)
contains exact installed versions and lockfile integrity values. npm packages used
only for building/testing are excluded from this renderer inventory; Node and
Electron are not application backend dependencies in this port.

| Package | Version | Upstream declared license |
| --- | --- | --- |
| @fontsource/lato | 5.3.0 | OFL-1.1 |
| @tauri-apps/api | 2.12.4 | Apache-2.0 OR MIT |
| js-tokens | 4.0.0 | MIT |
| loose-envify | 1.4.0 | MIT |
| object-assign | 4.1.1 | MIT |
| prop-types | 15.8.1 | MIT |
| react | 19.2.0 | MIT |
| react-dom | 19.2.0 | MIT |
| react-feather | 2.0.10 | MIT |
| react-is | 16.13.1 | MIT |
| scheduler | 0.27.0 | MIT |

## Retained timezone data and platform components

`crates/rize-service/src/windows_zones.json` retains the Windows-to-IANA timezone
mapping distributed as `windowsZones.json` by **node-ical 0.27.3**. Its parsed data
is unchanged; JSON formatting differs. node-ical declares Apache-2.0 and its package
author metadata names Jens Maus. Its exact supplied [Apache license](docs/licenses/node-ical-LICENSE.txt)
is preserved. node-ical's JavaScript runtime is not used by the Rust service.

The Windows desktop uses the operating system's Microsoft WebView2 runtime.
That runtime and any Microsoft bootstrapper retain Microsoft's own terms and
notices. They are platform components outside the Cargo/npm source inventories.
Installer tool outputs must retain their supplied notices as well.

Regenerate these inventories after lockfile changes with
`python scripts/license-notices.py` (Python 3.11+), using the release's Rust toolchain and an
installed npm graph (`npm ci`). The script reads actual downloaded crate archives
and installed package files; it records absent upstream texts instead of creating
attribution. It does not select an overall license for this project.

## Archived v1.2 notices

The following text and the other existing files in `docs/licenses` are retained
from the original Electron release for provenance. **This archival section
describes that earlier release, not the dependency graph or runtime shipped by
rs-rize.** The current inventories above supersede its package list.

<!-- legacy-v1.2-notices:start -->
Rize Local is an independent recreation. Rize’s name and the unchanged public favicon remain associated with their owner. Favicon provenance is recorded in docs/native-asset.txt. No proprietary Rize application bundle is included.

Full license texts for bundled fonts, icons, and renderer dependencies are reproduced below. Electron’s LICENSE and LICENSES.chromium.html and the native runtime packages’ license files are also retained in the Windows portable folder. Build-time dependency versions are in package-lock.json.

## React

```text
MIT License

Copyright (c) Meta Platforms, Inc. and affiliates.

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

## React DOM

```text
MIT License

Copyright (c) Meta Platforms, Inc. and affiliates.

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

## React Feather

```text
MIT License

Copyright (c) 2017 Carmelo Pullara

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

## Lato (SIL Open Font License 1.1)

```text
Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-HairlineItalic.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-Light.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-LightItalic.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-Regular.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-Italic.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-Bold.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-BoldItalic.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-Black.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1. Lato-BlackItalic.ttf: Copyright (c) 2010-2011 by tyPoland Lukasz Dziedzic (team@latofonts.com) with Reserved Font Name "Lato". Licensed under the SIL Open Font License, Version 1.1.

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

## Active Window

```text
MIT License

Copyright (c) 2024 Paymo LLC

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

## node-gyp-build

```text
The MIT License (MIT)

Copyright (c) 2017 Mathias Buus

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

## Installed production dependency inventory

The following installed versions are the production dependency graph from `package.json`,
including transitive runtime dependencies and fonts/renderer packages. Build-only
dependencies are excluded. Each linked notice was copied from that installed
package’s actual license file. Original notices above are retained. The full
license directory and machine-readable [inventory](docs/licenses/production-dependencies.json)
are distributed with the source and packaged app.

| Package | Version | Declared license | License / notice |
| --- | --- | --- | --- |
| @fontsource/lato | 5.3.0 | OFL-1.1 | [License text](docs/licenses/fontsource-lato-5.3.0.txt) |
| @paymoapp/active-window | 2.2.0 | MIT | [License text](docs/licenses/paymoapp-active-window-2.2.0.txt) |
| argparse | 2.0.1 | Python-2.0 | [License text](docs/licenses/argparse-2.0.1.txt) |
| builder-util-runtime | 9.7.0 | MIT | [License text](docs/licenses/builder-util-runtime-9.7.0.txt) |
| debug | 4.4.3 | MIT | [License text](docs/licenses/debug-4.4.3.txt) |
| electron-updater | 6.8.9 | MIT | [License text](docs/licenses/electron-updater-6.8.9.txt) |
| fs-extra | 10.1.0 | MIT | [License text](docs/licenses/fs-extra-10.1.0.txt) |
| graceful-fs | 4.2.11 | ISC | [License text](docs/licenses/graceful-fs-4.2.11.txt) |
| js-tokens | 4.0.0 | MIT | [License text](docs/licenses/js-tokens-4.0.0.txt) |
| js-yaml | 4.3.2 | MIT | [License text](docs/licenses/js-yaml-4.3.2.txt) |
| jsonfile | 6.2.1 | MIT | [License text](docs/licenses/jsonfile-6.2.1.txt) |
| lazy-val | 1.0.5 | MIT | [Upstream metadata; no license file](docs/licenses/lazy-val-1.0.5.txt) |
| lodash.escaperegexp | 4.1.2 | MIT | [License text](docs/licenses/lodash.escaperegexp-4.1.2.txt) |
| lodash.isequal | 4.5.0 | MIT | [License text](docs/licenses/lodash.isequal-4.5.0.txt) |
| loose-envify | 1.4.0 | MIT | [License text](docs/licenses/loose-envify-1.4.0.txt) |
| ms | 2.1.3 | MIT | [License text](docs/licenses/ms-2.1.3.txt) |
| node-gyp-build | 4.8.4 | MIT | [License text](docs/licenses/node-gyp-build-4.8.4.txt) |
| node-ical | 0.27.3 | Apache-2.0 | [License text](docs/licenses/node-ical-0.27.3.txt) |
| object-assign | 4.1.1 | MIT | [License text](docs/licenses/object-assign-4.1.1.txt) |
| prop-types | 15.8.1 | MIT | [License text](docs/licenses/prop-types-15.8.1.txt) |
| react | 19.2.0 | MIT | [License text](docs/licenses/react-19.2.0.txt) |
| react-dom | 19.2.0 | MIT | [License text](docs/licenses/react-dom-19.2.0.txt) |
| react-feather | 2.0.10 | MIT | [License text](docs/licenses/react-feather-2.0.10.txt) |
| react-is | 16.13.1 | MIT | [License text](docs/licenses/react-is-16.13.1.txt) |
| rrule-temporal | 2.2.8 | MIT | [License text](docs/licenses/rrule-temporal-2.2.8.txt) |
| sax | 1.6.1 | BlueOak-1.0.0 | [License text](docs/licenses/sax-1.6.1.txt) |
| scheduler | 0.27.0 | MIT | [License text](docs/licenses/scheduler-0.27.0.txt) |
| semver | 7.7.4 | ISC | [License text](docs/licenses/semver-7.7.4.txt) |
| temporal-polyfill | 1.0.6 | MIT | [License text](docs/licenses/temporal-polyfill-1.0.6.txt) |
| temporal-spec | 1.0.1 | Apache-2.0 | [License text](docs/licenses/temporal-spec-1.0.1.txt) |
| temporal-utils | 1.0.3 | MIT | [License text](docs/licenses/temporal-utils-1.0.3.txt) |
| tiny-typed-emitter | 2.1.0 | MIT | [License text](docs/licenses/tiny-typed-emitter-2.1.0.txt) |
| universalify | 2.0.1 | MIT | [License text](docs/licenses/universalify-2.0.1.txt) |

`lazy-val` 1.0.5 declares MIT in its upstream `package.json` but its installed
archive supplies no standalone license or copyright notice. Its exact relevant
metadata is preserved in the linked file rather than inventing attribution.

Electron is the packaged application runtime rather than an npm production
dependency here. The official Electron runtime’s `LICENSE` and
`LICENSES.chromium.html` must accompany the executable, covering Electron, Chromium,
Node.js and their included components. NSIS packaging and electron-builder are
build tools, not included in the production npm graph above. Their packaging
outputs retain their own runtime/installer notices as supplied by those tools.
<!-- legacy-v1.2-notices:end -->
