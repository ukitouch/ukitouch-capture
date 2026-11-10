# ukitouch-capture
Native extension of [ukitouch](https://github.com/ukitouch/ukitouch): capture, provides screen capture. Currently supported platforms: Windows, X11

## Build
Build release:
```bash
cargo build --release
```

Pack:
```bash
cargo xtask dist
```

will generate `ukitouch-capture-<platform>.zip` in dist/, can be directly installed in ukitouch.