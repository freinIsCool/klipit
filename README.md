# THIS APP IS IN BETA!
it is functional but does **not** have a gui
nor a settings gui/file (to adjust them you need to know rust)

<img width="267" height="150" alt="klipit" src="https://github.com/user-attachments/assets/08f8d359-c9cc-4633-b534-813b8385a7f4" />

an open-source [medal.tv](https://medal.tv/) wayland-native and linux alternative

### notes:
1. you do **not** get the perks of medal.tv in games (partnerships)
2. klipit does **not** have a "clips db"
3. the default codec is h264_nvenc (nvidia only) for amd see [AMD.txt](https://github.com/freinIsCool/klipit/blob/main/AMD.txt)


## building from source
yay!

requirements:
* `pipewire`
* `rust`
* `cargo`

```bash
# 1 clone the repo
git clone https://github.com/freinIsCool/klipit.git

# build
cargo build

# the built binary is located in cargo/target/debug/klipit
```
