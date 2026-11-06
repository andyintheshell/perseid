# Bundled fonts

- `Selawik-Regular.ttf`: UI font, licensed under the SIL OFL in `Selawik-OFL.txt`.
- `DroidSansFallback.ttf`: CJK fallback for proportional and monospace text,
  licensed under Apache-2.0. The upstream copyright notice and full license are
  preserved in `DroidSansFallback-NOTICE.txt`.

The Droid font is an unmodified copy from the Raspberry Pi distribution's
Android font repository, pinned to commit
`82e0fe7c16770b14c1685c8fe08b9e3edd817f4c`:

https://github.com/RPi-Distro/fonts-android/blob/82e0fe7c16770b14c1685c8fe08b9e3edd817f4c/DroidSansFallback.ttf

SHA-256: `05d71b179ef97b82cf1bb91cef290c600a510f77f39b4964359e3ef88378c79d`.

This variant covers the Chinese, Japanese, and Korean regression-test samples;
the separately named Full variant does not cover the Korean sample.
It replaces the much larger Noto Sans CJK font to keep the published crate below
crates.io's default 10 MiB compressed upload limit. It is not a coverage-equivalent
replacement for Noto: uncommon characters may still require another font.
