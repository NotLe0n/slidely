# slidely

slidely is a desktop app for studying recorded lectures. A project holds a
lecture's slides (PDFs) and its recordings (videos). slidely transcribes the
videos on your own machine with [Whisper](https://github.com/ggml-org/whisper.cpp),
using the GPU through Vulkan. It then matches what the lecturer says to the text
of the slides. After that you can jump from any slide to the point in the video
where it's discussed, and back again. You can also have the video follow the
slides you scroll to ("Video Follows Slides"), or the slides follow the video
("Slides Follow Video").

## Models

Transcription needs a Whisper model in ggml format. slidely looks for one in
`models/`, relative to the directory you start it from, in this order:

- `models/ggml-small-q5_1.bin` (recommended)
- `models/ggml-small.bin`
- `models/ggml-large-v3-turbo-q5_0.bin`

All of them can be downloaded from
[huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp/tree/main).

You can also add the Silero VAD model, `models/ggml-silero-v5.1.2.bin`, from
[huggingface.co/ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad/tree/main).
It's optional: with it, slidely skips the silent parts of a recording, so
transcription is faster.

```sh
mkdir -p models
curl -L -o models/ggml-small-q5_1.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin
curl -L -o models/ggml-silero-v5.1.2.bin https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin
```

To keep the models somewhere else, set `SLIDELY_WHISPER_MODEL` and
`SLIDELY_VAD_MODEL` to their paths.

## Installation

slidely is built from source with [Rust](https://rustup.rs) 1.85 or newer. It
needs GStreamer (for video), Vulkan (Metal on macOS), CMake and a C/C++
compiler (for whisper.cpp).

### Linux

Install the dependencies. On Debian/Ubuntu:

```sh
sudo apt install build-essential cmake clang pkg-config \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
  libvulkan-dev glslc \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libfontconfig-dev libssl-dev
```

Other distributions have the same packages under similar names: the GStreamer
development packages and plugins, the Vulkan headers and loader, `glslc`
(often packaged as `shaderc`), and the X11/Wayland/xkbcommon development
headers.

Then build and run it:

```sh
git clone https://github.com/Sillyhaus-GmbH/slidely.git
cd slidely
# download the models into models/ (see above)
cargo run --release
```

### macOS

Install Xcode (for Metal, which whisper.cpp uses on macOS) and
[Homebrew](https://brew.sh), then the dependencies:

```sh
brew install gstreamer pkgconf cmake
```

Then build and run it:

```sh
git clone https://github.com/Sillyhaus-GmbH/slidely.git
cd slidely
# download the models into models/ (see above)
cargo run --release
```

The prebuilt macOS release needs Homebrew's `gstreamer` too. macOS blocks
binaries downloaded from the internet that aren't signed; allow it with
`xattr -d com.apple.quarantine slidely` in the unpacked folder.

### Windows

On Windows, slidely is built for the `x86_64-pc-windows-gnu` target with
[MSYS2](https://www.msys2.org). `.cargo/config.toml` expects MSYS2 to be
installed at `C:\msys64`.

1. Install Rust with the GNU toolchain:

   ```sh
   rustup default stable-x86_64-pc-windows-gnu
   ```

2. Install MSYS2 to `C:\msys64`. In its **UCRT64** shell, run:

   ```sh
   pacman -Syu
   pacman -S mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-clang \
     mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-gstreamer mingw-w64-ucrt-x86_64-gst-plugins-base \
     mingw-w64-ucrt-x86_64-gst-plugins-good mingw-w64-ucrt-x86_64-gst-plugins-bad mingw-w64-ucrt-x86_64-gst-libav \
     mingw-w64-ucrt-x86_64-vulkan-headers mingw-w64-ucrt-x86_64-vulkan-loader mingw-w64-ucrt-x86_64-shaderc
   ```

3. Add `C:\msys64\ucrt64\bin` to your `PATH`, ahead of any other MinGW
   installation. This is needed both to build and to run slidely. If gcc comes
   from a different MinGW distribution (e.g. MinGW-Builds) than the DLLs,
   whisper.cpp crashes at startup with `STATUS_ENTRYPOINT_NOT_FOUND`
   (`0xc0000139`). Don't set `PKG_CONFIG_PATH` to another MSYS2 environment
   (such as `mingw64`): linking then fails with undefined references to
   `std::codecvt`.

4. Build and run it:

   ```sh
   git clone https://github.com/Sillyhaus-GmbH/slidely.git
   cd slidely
   # download the models into models/ (see above)
   cargo run --release
   ```

## Usage

1. Start slidely from the repository root, so it finds `models/`.
2. Create a project with **File → New Project**.
3. In **View → Manage Project…**, add the lecture's PDFs and videos, then
   transcribe the videos.
4. When a video is transcribed, its slides are matched to it.
