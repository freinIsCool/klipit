# THIS APP IS IN BETA!
it is functional but does **not** have a gui
nor a settings gui/file (to adjust them you need to know rust)

# klipit
an open-source [medal.tv](https://medal.tv/) wayland-native and linux alternative

### notes:
1. you do **not** get the perks of medal.tv in games (partnerships)
2. klipit does **not** have a "clips db"
3. the default codec is h264_nvenc (nvidia only) for amd see PLACEHOLDER


## building from source
yay!

requirements:
* `ashpd`
* `ctrlc`
* `ez-ffmpeg`
* `futures-util`
* `libspa`
* `pipewire` (crate and package)
* `tokio`
* `rust`
* `cargo`

```bash
# 1 clone the repo
git clone https://github.com/freinIsCool/klipit.git

# build
cargo build

# the built binary is located in cargo/target/debug/klipit
```
