//! Windows capture via GDI.

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetObjectW, ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, GetSystemMetrics, CURSORINFO, CURSOR_SHOWING,
    DI_NORMAL, HCURSOR, ICONINFO, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

use super::{
    CaptureBackend, CaptureError, CaptureOptions, Cursor, CursorImage, Damage, ScreenInfo,
};

const BPP: usize = 4;

/// Tile size for diffing.
const TILE: usize = 64;

/// [left, top, right, bottom]
type Bounds = [usize; 4];

/// Maximum extra pixels allowed when merging damage rectangles.
const MERGE_BUDGET: usize = 100_000;

#[derive(Debug)]
struct Pointer {
    handle: HCURSOR,
    x: i32,
    y: i32,
    xhot: i32,
    yhot: i32,
}

#[derive(Debug, PartialEq, Eq)]
struct Shape {
    width: u16,
    height: u16,
    xhot: u16,
    yhot: u16,
    /// Row-major, premultiplied RGBA.
    pixels: Vec<u8>,
}

pub struct WindowsCapture {
    /// Desktop DC, memory DC, and the original bitmap selected into it.
    screen: HDC,
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    base: *mut u8,
    /// Virtual desktop origin (may be negative on multi-monitor setups).
    x0: i32,
    /// Virtual desktop origin (may be negative on multi-monitor setups).
    y0: i32,
    width: u16,
    height: u16,
    len: usize,
    /// Previous frame for diffing.
    prev: Vec<u8>,
    /// Current frame copied from the DIB.
    cur: Vec<u8>,
    /// Dirty tile flags.
    dirty: Vec<bool>,
    /// Tiles already assigned to an output rectangle.
    covered: Vec<bool>,
    /// Force a full-frame emit on next poll.
    force_full: bool,
    /// Cursor display mode.
    cursor: Cursor,
    /// Cached cursor handle and hotspot.
    cursor_hotspot: Option<(HCURSOR, i32, i32)>,
    /// Cached cursor image.
    cursor_shape: Option<Shape>,
    /// Output buffer reused across polls.
    out: Vec<(Damage, Vec<u8>)>,
}

impl WindowsCapture {
    pub fn new() -> Result<Self, CaptureError> {
        Self::new_on().map_err(|reason| CaptureError::Unsupported { reason })
    }

    fn new_on() -> Result<Self, String> {
        let x0 = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let y0 = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        let size = 1..=u16::MAX as i32;
        if !size.contains(&width) || !size.contains(&height) {
            return Err(format!("unusable desktop size {width}x{height}"));
        }
        let (width, height) = (width as u16, height as u16);
        let len = width as usize * height as usize * BPP;

        let screen = unsafe { GetDC(null_mut()) };
        if screen.is_null() {
            return Err(format!("GetDC: {}", std::io::Error::last_os_error()));
        }

        let dc = unsafe { CreateCompatibleDC(screen) };
        if dc.is_null() {
            unsafe { ReleaseDC(null_mut(), screen) };
            return Err(format!(
                "CreateCompatibleDC: {}",
                std::io::Error::last_os_error()
            ));
        }

        // Negative height requests a top-down DIB.
        let mut bmi: BITMAPINFO = unsafe { zeroed() };
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width as i32;
        bmi.bmiHeader.biHeight = -(height as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut base = null_mut();
        let dib = unsafe { CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut base, null_mut(), 0) };
        if dib.is_null() || base.is_null() {
            unsafe {
                DeleteDC(dc);
                ReleaseDC(null_mut(), screen);
            }
            return Err(format!(
                "CreateDIBSection: {}",
                std::io::Error::last_os_error()
            ));
        }
        let old = unsafe { SelectObject(dc, dib as HGDIOBJ) };

        let tiles = (width as usize).div_ceil(TILE) * (height as usize).div_ceil(TILE);
        Ok(Self {
            screen,
            dc,
            dib,
            old,
            base: base as *mut u8,
            x0,
            y0,
            width,
            height,
            len,
            prev: vec![0u8; len],
            cur: vec![0u8; len],
            dirty: vec![false; tiles],
            covered: vec![false; tiles],
            force_full: true,
            cursor: Cursor::Off,
            cursor_hotspot: None,
            cursor_shape: None,
            out: Vec::new(),
        })
    }

    /// Cursor state if visible and compositing is enabled.
    fn pointer(&mut self) -> Option<Pointer> {
        if self.cursor != Cursor::Composite {
            return None;
        }

        let ci = visible_cursor()?;
        if self.cursor_hotspot.map(|(h, _, _)| h) != Some(ci.hCursor) {
            let mut ii: ICONINFO = unsafe { zeroed() };
            if unsafe { GetIconInfo(ci.hCursor, &mut ii) } == 0 {
                return None;
            }
            free_icon_info(&ii);
            self.cursor_hotspot = Some((ci.hCursor, ii.xHotspot as i32, ii.yHotspot as i32));
        }

        let (_, xhot, yhot) = self.cursor_hotspot?;
        Some(Pointer {
            handle: ci.hCursor,
            x: ci.ptScreenPos.x - self.x0,
            y: ci.ptScreenPos.y - self.y0,
            xhot,
            yhot,
        })
    }

    /// Draw the cursor into the frame.
    fn composite(&self, p: &Pointer) {
        unsafe {
            DrawIconEx(
                self.dc,
                p.x - p.xhot,
                p.y - p.yhot,
                p.handle,
                0,
                0,
                0,
                null_mut(),
                DI_NORMAL,
            );
        }
    }

    /// Grab the desktop into the DIB.
    fn blit(&mut self) -> Result<(), CaptureError> {
        let ok = unsafe {
            BitBlt(
                self.dc,
                0,
                0,
                self.width as i32,
                self.height as i32,
                self.screen,
                self.x0,
                self.y0,
                SRCCOPY,
            )
        };
        if ok == 0 {
            return Err(self.fail(format!("BitBlt: {}", std::io::Error::last_os_error())));
        }

        Ok(())
    }

    /// Copy DIB pixels into `cur`.
    fn read(&mut self) {
        let src = unsafe { std::slice::from_raw_parts(self.base, self.len) };
        self.cur.copy_from_slice(src);
    }

    fn fail(&self, reason: String) -> CaptureError {
        CaptureError::Unsupported { reason }
    }

    /// Build damage rectangles from dirty tiles.
    fn diff(&mut self) {
        self.out.clear();
        let stride = self.width as usize * BPP;
        let tiles_x = (self.width as usize).div_ceil(TILE);
        let tiles_y = (self.height as usize).div_ceil(TILE);
        self.dirty.fill(false);

        for ty in 0..tiles_y {
            let y0 = ty * TILE;
            let y1 = (y0 + TILE).min(self.height as usize);
            for tx in 0..tiles_x {
                let x0 = tx * TILE;
                let x1 = ((tx + 1) * TILE).min(self.width as usize);
                let (b0, b1) = (x0 * BPP, x1 * BPP);
                for y in y0..y1 {
                    let row = y * stride;
                    if self.prev[row + b0..row + b1] != self.cur[row + b0..row + b1] {
                        self.dirty[ty * tiles_x + tx] = true;
                        break;
                    }
                }
            }
        }

        // grow right, then down
        self.covered.fill(false);
        let mut rects: Vec<Bounds> = Vec::new();
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let idx = ty * tiles_x + tx;
                if !self.dirty[idx] || self.covered[idx] {
                    continue;
                }

                let mut wide = 1;
                while tx + wide < tiles_x
                    && self.dirty[ty * tiles_x + tx + wide]
                    && !self.covered[ty * tiles_x + tx + wide]
                {
                    wide += 1;
                }

                let mut tall = 1;
                'down: while ty + tall < tiles_y {
                    for i in 0..wide {
                        let j = (ty + tall) * tiles_x + tx + i;
                        if !self.dirty[j] || self.covered[j] {
                            break 'down;
                        }
                    }
                    tall += 1;
                }

                for i in 0..wide {
                    for j in 0..tall {
                        self.covered[(ty + j) * tiles_x + tx + i] = true;
                    }
                }

                let x0 = tx * TILE;
                let y0 = ty * TILE;
                let x1 = (x0 + wide * TILE).min(self.width as usize);
                let y1 = (y0 + tall * TILE).min(self.height as usize);
                rects.push([x0, y0, x1, y1]);
            }
        }

        coalesce(&mut rects);

        for [x0, y0, x1, y1] in rects {
            let (w, h) = ((x1 - x0) as u16, (y1 - y0) as u16);
            // DIB is BGRA while callers expect RGBA. `A` here is opaque because the DIB
            // alpha byte is unused for desktop captures.
            let mut px = Vec::with_capacity(w as usize * h as usize * BPP);
            for y in y0..y1 {
                let row = y * stride;
                for x in x0..x1 {
                    let o = row + x * BPP;
                    px.push(self.cur[o + 2]);
                    px.push(self.cur[o + 1]);
                    px.push(self.cur[o]);
                    px.push(0xff);
                }
            }

            self.out.push((
                Damage {
                    x: x0 as u16,
                    y: y0 as u16,
                    width: w,
                    height: h,
                },
                px,
            ));
        }
    }

    /// Emit the full frame as a single rectangle.
    fn full(&mut self) {
        self.out.clear();
        let mut px = Vec::with_capacity(self.len);
        for chunk in self.cur.chunks_exact(BPP) {
            px.push(chunk[2]);
            px.push(chunk[1]);
            px.push(chunk[0]);
            px.push(0xff);
        }
        self.out.push((
            Damage {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            },
            px,
        ));
    }
}

/// Cursor info if the OS is currently showing one.
fn visible_cursor() -> Option<CURSORINFO> {
    let mut ci: CURSORINFO = unsafe { zeroed() };
    ci.cbSize = size_of::<CURSORINFO>() as u32;
    if unsafe { GetCursorInfo(&mut ci) } == 0 {
        return None;
    }
    if ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null() {
        return None;
    }
    Some(ci)
}

/// Free the bitmaps allocated by GetIconInfo.
fn free_icon_info(ii: &ICONINFO) {
    unsafe {
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask as HGDIOBJ);
        }
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor as HGDIOBJ);
        }
    }
}

fn bitmap_size(hbm: HBITMAP) -> Option<(usize, usize)> {
    if hbm.is_null() {
        return None;
    }

    let mut bm: BITMAP = unsafe { zeroed() };
    let got = unsafe {
        GetObjectW(
            hbm as HGDIOBJ,
            size_of::<BITMAP>() as i32,
            (&mut bm as *mut BITMAP).cast(),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }
    Some((bm.bmWidth as usize, bm.bmHeight as usize))
}

/// Read a bitmap as top-down BGRA.
fn read_bitmap(hbm: HBITMAP, width: usize, height: usize) -> Option<Vec<u8>> {
    if hbm.is_null() {
        return None;
    }

    let dc = unsafe { CreateCompatibleDC(null_mut()) };
    if dc.is_null() {
        return None;
    }

    let old = unsafe { SelectObject(dc, hbm as HGDIOBJ) };
    if old.is_null() {
        unsafe { DeleteDC(dc) };
        return None;
    }

    let mut bmi: BITMAPINFO = unsafe { zeroed() };
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut px = vec![0u8; width * height * BPP];
    let got = unsafe {
        GetDIBits(
            dc,
            hbm,
            0,
            height as u32,
            px.as_mut_ptr().cast(),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };

    unsafe {
        SelectObject(dc, old);
        DeleteDC(dc);
    }

    (got != 0).then_some(px)
}

fn read_shape(ii: &ICONINFO) -> Option<Shape> {
    let color = bitmap_size(ii.hbmColor);
    let mask = bitmap_size(ii.hbmMask);

    // Monochrome cursors store AND and XOR masks stacked vertically.
    let (width, height) = match color {
        Some(size) => size,
        None => (mask?.0, mask?.1 / 2),
    };
    if width == 0 || height == 0 {
        return None;
    }

    let mut px = match color {
        Some(_) => {
            let mut px = read_bitmap(ii.hbmColor, width, height)?;
            if !px.chunks_exact(BPP).any(|p| p[3] != 0) {
                // Alpha channel is empty, derive transparency from the 1-bpp mask.
                if let Some(m) = read_bitmap(ii.hbmMask, width, height) {
                    for (p, m) in px.chunks_exact_mut(BPP).zip(m.chunks_exact(BPP)) {
                        if m[0] | m[1] | m[2] == 0 {
                            p[3] = 0xff;
                        } else {
                            p.copy_from_slice(&[0, 0, 0, 0]);
                        }
                    }
                }
            }
            px
        }
        None => {
            let m = read_bitmap(ii.hbmMask, width, height * 2)?;
            let mut px = vec![0u8; width * height * BPP];
            for i in 0..width * height {
                if m[i * BPP] != 0 {
                    continue;
                }
                let v = if m[(i + width * height) * BPP] != 0 {
                    0xff
                } else {
                    0
                };
                px[i * BPP..i * BPP + 3].fill(v);
                px[i * BPP + 3] = 0xff;
            }
            px
        }
    };

    for p in px.chunks_exact_mut(BPP) {
        p.swap(0, 2);
    }

    Some(Shape {
        width: width as u16,
        height: height as u16,
        xhot: ii.xHotspot as u16,
        yhot: ii.yHotspot as u16,
        pixels: px,
    })
}

/// Extra pixels added by merging two rectangles.
fn added_area(a: &Bounds, b: &Bounds) -> usize {
    let union = area(&merge(a, b)) as isize;
    let parts = (area(a) + area(b)) as isize;
    (union - parts).max(0) as usize
}

fn area(r: &Bounds) -> usize {
    (r[2] - r[0]) * (r[3] - r[1])
}

fn merge(a: &Bounds, b: &Bounds) -> Bounds {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn covers(a: &Bounds, b: &Bounds) -> bool {
    a[0] <= b[0] && a[1] <= b[1] && b[2] <= a[2] && b[3] <= a[3]
}

/// Merge damage rectangles greedily while the wasted pixels stay under budget.
fn coalesce(rects: &mut Vec<Bounds>) {
    loop {
        let mut best: Option<(usize, Bounds)> = None;
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let cost = added_area(a, b);
                if best.as_ref().map_or(true, |(cheapest, _)| cost < *cheapest) {
                    best = Some((cost, merge(a, b)));
                }
            }
        }

        let Some((cost, union)) = best else { return };
        if cost > MERGE_BUDGET {
            return;
        }

        rects.retain(|r| !covers(&union, r));
        rects.push(union);
    }
}

impl CaptureBackend for WindowsCapture {
    fn info(&self) -> ScreenInfo {
        ScreenInfo {
            width: self.width,
            height: self.height,
        }
    }

    fn poll(&mut self) -> Result<Option<Vec<(Damage, Vec<u8>)>>, CaptureError> {
        let full = self.force_full;
        self.force_full = false;

        let pointer = self.pointer();

        self.blit()?;

        if let Some(p) = &pointer {
            self.composite(p);
        }
        self.read();

        if full {
            self.full();
        } else {
            self.diff();
        }

        if self.out.is_empty() {
            return Ok(None);
        }

        self.prev.copy_from_slice(&self.cur);
        Ok(Some(std::mem::take(&mut self.out)))
    }

    fn resync(&mut self) {
        self.force_full = true;
    }

    fn configure(&mut self, options: &CaptureOptions) -> bool {
        if options.cursor == self.cursor {
            return false;
        }

        self.cursor = options.cursor;
        // Cursor mode changed, repaint to clear or redraw the pointer.
        self.force_full = true;
        self.cursor_shape = None;
        false
    }

    fn cursor_image(&mut self) -> Result<Option<CursorImage>, CaptureError> {
        if self.cursor != Cursor::Raw {
            return Ok(None);
        }

        let Some(ci) = visible_cursor() else {
            return Ok(None);
        };
        let mut ii: ICONINFO = unsafe { zeroed() };
        if unsafe { GetIconInfo(ci.hCursor, &mut ii) } == 0 {
            return Ok(None);
        }
        let shape = read_shape(&ii);
        free_icon_info(&ii);

        let Some(shape) = shape else {
            return Ok(None);
        };
        if self.cursor_shape.as_ref() == Some(&shape) {
            return Ok(None);
        }
        let image = CursorImage {
            width: shape.width,
            height: shape.height,
            xhot: shape.xhot,
            yhot: shape.yhot,
            pixels: shape.pixels.clone(),
        };
        self.cursor_shape = Some(shape);
        Ok(Some(image))
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.dib as HGDIOBJ);
            DeleteDC(self.dc);
            ReleaseDC(null_mut(), self.screen);
        }
    }
}

unsafe impl Send for WindowsCapture {}
