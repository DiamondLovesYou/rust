# The Rust Programming Language

This is a compiler for Rust, including standard libraries, tools and
documentation. In contrast to
[Rust proper](https://github.com/rust-lang/rust.git), this Rust can target
PNaCl/NaCl, including ```le32-unknown-nacl```, ```x86_64-unknown-nacl```,
```i686-unknown-nacl```(WIP), and ```arm-unknown-nacl```(WIP).

## Quick Start

1. Download a [binary installer][installer] for your platform.
2. Read [The Rust Programming Language][trpl].
3. Enjoy!

If you'd like to cross compile to a (P)NaCl target, you'll need to build from
source. I (Richard Diamond) am working on creating some build infrastructure so
I may offer nightly downloads, however that isn't finished yet.

> ***Note:*** Windows users can read the detailed
> [using Rust on Windows][win-wiki] notes on the wiki.

[installer]: http://www.rust-lang.org/install.html
[trpl]: http://doc.rust-lang.org/book/index.html
[win-wiki]: https://github.com/rust-lang/rust/wiki/Using-Rust-on-Windows

## Building from Source

1. Make sure you have installed the dependencies:
    * `g++` 4.7 or `clang++` 3.x
    * `python` 2.6 or later (but not 3.x)
    * GNU `make` 3.81 or later
    * `curl`
    * `git`
    * `pepper_39` or newer from the [NaCl SDK](https://developer.chrome.com/native-client).
2. Download and build Rust:

    Or to build from the [repo] do:

        $ git clone https://github.com/DiamondLovesYou/rust.git
        $ cd rust

    Now that you have Rust's source code, you can configure and build it for
    PNaCl with (add `x86_64-unknown-nacl` if you'd like to target NaCl):

        $ ./configure --target=le32-unknown-nacl --nacl-cross-path=path/to/pepper_39
        $ make && make install

    > ***Note:*** You may need to use `sudo make install` if you do not normally have
    > permission to modify the destination directory. The install locations can
    > be adjusted by passing a `--prefix` argument to `configure`. Various other
    > options are also supported, pass `--help` for more information on them.

    When complete, `make install` will place several programs into
    `/usr/local/bin`: `rustc`, the Rust compiler, `rustdoc`, the
    API-documentation tool, and `rust-pnacl-trans`, the PNaCl bitcode to NaCl
    nexe translator (like `pnacl-trans`).
3. Read [The Rust Programming Language][trpl].
4. Compile with:

        $ NACL_SDK_ROOT=path/to/pepper rustc --target=le32-unknown-nacl

    to target PNaCl, or with:

        $ NACL_SDK_ROOT=path/to/pepper rustc --target=x86_64-unknown-nacl

    to target NaCl.

4. Enjoy!

### Building on Windows

> ***Note:*** building on Windows is not tested by me (Richard Diamond)

To easily build on windows we can use [MSYS2](http://msys2.github.io/):

1. Grab the latest MSYS2 installer and go through the installer.
2. Now from the MSYS2 terminal we want to install the mingw64 toolchain and the other
   tools we need.

```bash
# choose one based on platform
$ pacman -S mingw-w64-i686-toolchain
$ pacman -S mingw-w64-x86_64-toolchain

$ pacman -S base-devel
```

3. With that now start `mingw32_shell.bat` or `mingw64_shell.bat`
   from where you installed MSYS2 (i.e. `C:\msys`). Which one you
   choose depends on if you want 32 or 64 bit Rust.
4. From there just navigate to where you have Rust's source code, configure and build it:

        $ ./configure
        $ make && make install

[repo]: https://github.com/rust-lang/rust
[tarball]: https://static.rust-lang.org/dist/rust-nightly.tar.gz
[trpl]: http://doc.rust-lang.org/book/index.html

## Notes

Since the Rust compiler is written in Rust, it must be built by a
precompiled "snapshot" version of itself (made in an earlier state of
development). As such, source builds require a connection to the Internet, to
fetch snapshots, and an OS that can execute the available snapshot binaries.

Snapshot binaries are currently built and tested on several platforms:

* Windows (7, 8, Server 2008 R2), x86 and x86-64 (64-bit support added in Rust 0.12.0)
* Linux (2.6.18 or later, various distributions), x86 and x86-64
* OSX 10.7 (Lion) or greater, x86 and x86-64

You may find that other platforms work, but these are our officially
supported build environments that are most likely to work.

Rust currently needs about 1.5 GiB of RAM to build without swapping; if it hits
swap, it will take a very long time to build.

There is a lot more documentation in the [wiki].

[wiki]: https://github.com/rust-lang/rust/wiki

## PNaCl/NaCl Notes

Some helpful projects:

* [ppapi](https://github.com/DiamondLovesYou/rust-ppapi) - Rust idiomatic
  bindings to Pepper. If you're using this fork, you likely also want this
  crate.
* [ppapi-tester](https://github.com/DiamondLovesYou/rust-ppapi) - Integration
  test wrapper. Replaces tests at compile time with a wrapper which cross
  compiles your real test to PNaCl, then runs it inside an independent Chrome
  instance.
* [pexe-runner](https://github.com/DiamondLovesYou/pexe-runner) (Linux only) -
  Allows you to run your pexes like you would a regular binary, except under the
  NaCl sandbox. Ie: ```./pexe-bitcode.pexe $ARGS```.

## Getting help and getting involved

The Rust community congregates in a few places:

* [StackOverflow] - Get help here.
* [/r/rust] - General discussion.
* [discuss.rust-lang.org] - For development of the Rust language itself.

[StackOverflow]: http://stackoverflow.com/questions/tagged/rust
[/r/rust]: http://reddit.com/r/rust
[discuss.rust-lang.org]: http://discuss.rust-lang.org/

## License

Rust is primarily distributed under the terms of both the MIT license
and the Apache License (Version 2.0), with portions covered by various
BSD-like licenses.

See LICENSE-APACHE, LICENSE-MIT, and COPYRIGHT for details.
