cian — building from source on an offline Windows machine
=========================================================

This package is the whole of cian: the source, and every crate it depends
on, already downloaded. Nothing in a build of it reaches the network.

It is the package to bring in-house when the point is to change cian, not
just to run it. To only run it, take cian-windows-x64.zip instead — that
one is already built.


What has to be on the machine first
-----------------------------------

That depends on which half you mean to change, and the two are very
different sizes.

**To work on the Electron front end, nothing.** Not Rust, not npm.
The front end is JavaScript; it talks to a prebuilt cian-server.exe over
a pipe. Take that one file (9 MB, `cian-server-win-x64.exe` on the
releases page) and a standalone Electron, and you can edit and restart
all day. See "The Electron front end" below.

Measured, v1.1.0, so that what to carry in can be decided before
carrying it:

    cian-tui.exe              13 MB   the terminal build, one file
    cian-server.exe            9 MB   the engine, one file
    cian-windows-x64.zip      25 MB   both, compressed, plus docs

    gui\ + gui\vendor\        29 MB   the Electron front end's own files
    Electron itself          247 MB   unzipped; ~100 MB as its own zip

**So the Electron front end is not "an exe".** It is about 285 MB of
files, or roughly 110 MB zipped, because Chromium comes with it. If the
constraint is what fits on the way in, cian-tui.exe is one file and
cian-server.exe is one file; the Electron build is a folder.

**To build the Rust side**, one thing or four, depending on which
programs you want:

  cian-server.exe alone — the engine, and all the Electron front end
  needs — compiles no C at all. Its ninety crates are pure Rust. So the
  GNU toolchain, which brings its own linker, is enough on its own:

     rust-<version>-x86_64-pc-windows-gnu.msi   (about 350 MB)

     From https://forge.rust-lang.org/infra/other-installation-methods.html
     — the standalone .msi, not rustup-init.exe, which wants the network.
     BUILT-WITH.txt records the version this package was verified with.

  cian-tui.exe needs three more, because it carries SFTP and Lua, and
  those build C:

  1. Rust, MSVC flavour this time
       rust-<version>-x86_64-pc-windows-msvc.msi  (about 290 MB)
  2. Visual Studio Build Tools — the C/C++ compiler. **Several GB**, and
       the reason the list above is worth reading first. Offline layout:
          vs_BuildTools.exe --layout C:\vslayout ^
            --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended ^
            --lang en-US ja-JP
       Run that where there is network, carry C:\vslayout across.
  3. CMake            https://cmake.org/download/  (the .msi)
  4. NASM             https://www.nasm.us/          (the .exe installer)

       3 and 4 are for aws-lc-sys, the cryptography behind SFTP. It is
       the one dependency that builds a C library through CMake, and on
       x86_64 Windows its assembly goes through NASM. Both must be on
       PATH — open a new terminal after installing and check:
          cmake --version
          nasm -v

       If a build fails somewhere inside aws-lc-sys, one of these two is
       the reason nine times out of ten.


Building
--------

For the engine alone, on the GNU toolchain, any command prompt will do:

    cargo build --release --offline -p cian-server

For everything, open "x64 Native Tools Command Prompt for VS" — the one
with the MSVC environment already set — cd into this folder, and:

    cargo build --release --offline

    --offline is not optional politeness. It tells cargo to use the
    crates in vendor\ and to fail loudly rather than quietly trying to
    reach crates.io. If the build succeeds with it, the build needs no
    network at all.

What lands in target\release\ :

    cian-tui.exe     the terminal build
    cian-server.exe  the engine the Electron front end talks to

Running the tests:

    cargo test --workspace --offline


The Electron front end
----------------------

`gui\` is a second front end that draws through Chromium instead of the
terminal. It is what this package is usually brought in-house for.

**It still needs no npm.** What it loads beyond Node's own modules — the
editor, the diagram drawer, the font — is already unpacked in `gui\vendor\`
in this package, which is the whole reason that folder is here. Nothing in
running it reaches the registry or the network.

What you have to supply is Electron itself:

    electron-v33.4.11-win32-x64.zip
    https://github.com/electron/electron/releases

Unzip that anywhere, put cian-server.exe beside gui\ (or build it), and
double-click:

    gui\run.bat

It looks for Electron in the places it is usually put — $CIAN_ELECTRON, a
distribution unzipped next to the repository, a copy under node_modules — and
says which places it looked in when it finds none. It also refuses to open an
empty window when the engine is missing, and says which of the two ways to
supply it.

To point it somewhere specific:

    set CIAN_ELECTRON=C:\path\to\electron-v33.4.11-win32-x64\electron.exe

The long way still works, and is what run.bat does:

    <where you unzipped>\electron.exe <this folder>\gui

No `npm install`, which would reach the network, and no `npm start`, which
would want node_modules to exist.

The engine is found automatically: gui\engine.js looks beside itself first,
then in target\release\ and target\debug\, **newest wins rather than
release wins** — a morning's release build sitting beside an afternoon of
`cargo build` is how you end up talking to yesterday's engine.

Editing gui\*.js or index.html and restarting Electron is the whole
development loop. Nothing is compiled.


What is in gui\vendor\, and when you have to rebuild it
-------------------------------------------------------

    monaco\          The editor: what F3 opens files in.
    monaco-vim.js    The vim grammar over it.
    mermaid.js       Diagrams in the Markdown preview.
    fonts\cian.ttf   HackGen Console NF — Japanese, monospaced, with the
                     Nerd Font glyphs the listing draws its icons from.

**The font is not decoration.** Without it the listing falls back to whatever
the machine has, which on a Mac is a proportional face — columns stop lining
up, the shell grid shears, and the icons come out as boxes. It is the same
file the Electron front end draws the listing with.

None of this is committed to the repository — several megabytes that never
change do not belong in every clone — so a *checkout* has to build it once,
on a machine with a network:

    cd gui
    npm install --omit=dev --no-audit --no-fund
    node vendor.js

`vendor.js` copies out of node_modules and looks for the font in, in order:
$CIAN_FONT, vendor-font\cian.ttf (where the release workflow puts it), and
the usual system locations. When it finds none it says every place it
looked and carries on — the window still runs, it just is not laid out on a
grid.

**In this package all of that is already done.** You only need it if you take
a fresh clone instead.


How the offline part works
--------------------------

    vendor\              Every crate, unpacked. About six hundred of them.
    .cargo\config.toml   Tells cargo to read vendor\ instead of crates.io.

Both are already in place. Do not delete .cargo\config.toml — without it
cargo ignores vendor\ entirely and tries the network.


Changing cian
-------------

Editing the source and rebuilding needs nothing further: the crates are
all here.

Adding or upgrading a dependency does need the network, because the new
crate is not in vendor\. Do that on a machine that has one:

    cargo add <crate>            # or edit Cargo.toml
    cargo vendor --versioned-dirs vendor > .cargo/config.toml

and bring the whole folder back across. There is no way around this; a
crate that has never been downloaded has to be downloaded once.


Contents
--------

    Cargo.toml, Cargo.lock, crates\   The source.
    vendor\                            Its dependencies.
    .cargo\config.toml                 The redirect that makes them count.
    vendor-font\cian.ttf               The bundled font. Normally fetched
                                       during a build; carried here instead.
    gui\vendor\                        The editor, the diagram drawer and
                                       the same font, for the Electron front
                                       end. Normally built by
                                       `node gui/vendor.js` against
                                       node_modules; carried here instead.
    examples\init.lua                  A starter configuration.
    packaging\windows\install.ps1      Puts a built exe on PATH.
    gui\run.bat                        Starts the Electron front end.
    BUILT-WITH.txt                     The compiler and commit this was
                                       vendored from.
    README.md / README.ja.md           The manual.
