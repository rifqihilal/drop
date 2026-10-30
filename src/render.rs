//! Draws the notch with Direct2D into a DIB and pushes it to the layered window.
//!
//! Rendering is software-only on purpose: the surface is tiny, and skipping a
//! Direct3D device keeps the GPU driver out of the process, which is most of
//! what makes a webview-based notch heavy.

use std::ffi::c_void;

use windows::core::{w, Result};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_SMALL, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_SWEEP_DIRECTION_CLOCKWISE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use windows_numerics::Vector2;

use crate::anim::IDLE;

/// Width of the concave curves joining the notch to the top screen edge.
const EAR: f32 = 10.0;
/// The canvas fits the largest notch shape; everything outside it is transparent.
pub const CANVAS: (f32, f32) = (IDLE.0 + 2.0 * EAR, IDLE.1);

const EYE_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.93,
    g: 0.96,
    b: 1.0,
    a: 1.0,
};

/// Everything the renderer needs for one frame, in logical pixels.
pub struct Scene<'a> {
    pub width: f32,
    pub height: f32,
    pub eyes_visible: f32,
    pub blink: f32,
    pub look: f32,
    pub clock: &'a str,
}

pub struct Renderer {
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    text: IDWriteTextFormat,
    black: ID2D1SolidColorBrush,
    light: ID2D1SolidColorBrush,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    size: SIZE,
}

impl Renderer {
    pub fn new(scale: f32) -> Result<Self> {
        unsafe {
            let size = SIZE {
                cx: (CANVAS.0 * scale).ceil() as i32,
                cy: (CANVAS.1 * scale).ceil() as i32,
            };

            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.cx,
                    // Negative height: top-down rows, matching Direct2D.
                    biHeight: -size.cy,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let previous = SelectObject(dc, bitmap.into());

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                // Drawing happens in logical pixels; Direct2D scales to the DPI.
                dpiX: 96.0 * scale,
                dpiY: 96.0 * scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = factory.CreateDCRenderTarget(&props)?;
            target.BindDC(
                dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: size.cx,
                    bottom: size.cy,
                },
            )?;

            let black = target.CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0), None)?;
            let light = target.CreateSolidColorBrush(&EYE_COLOR, None)?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let text = dwrite.CreateTextFormat(
                w!("Segoe UI Variable Display"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                13.0,
                w!("id-id"),
            )?;
            text.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            text.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

            Ok(Self {
                factory,
                target,
                text,
                black,
                light,
                dc,
                bitmap,
                previous,
                size,
            })
        }
    }

    /// Size of the window in physical pixels.
    pub fn size(&self) -> SIZE {
        self.size
    }

    pub fn present(&self, hwnd: HWND, origin: POINT, scene: &Scene) -> Result<()> {
        unsafe {
            let t = &self.target;
            t.BeginDraw();
            t.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            self.draw_shape(scene)?;
            self.draw_eyes(scene);
            self.draw_clock(scene);
            t.EndDraw(None, None)?;

            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&origin),
                Some(&self.size),
                Some(self.dc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }

    /// Black body with rounded bottom corners and concave ears at the top edge.
    unsafe fn draw_shape(&self, scene: &Scene) -> Result<()> {
        let cx = CANVAS.0 / 2.0;
        let (w, h) = (scene.width, scene.height);
        let (x0, x1) = (cx - w / 2.0, cx + w / 2.0);
        let r = (h / 2.0).min(24.0);
        let ear = EAR.min(h - r);

        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(x0 - ear, 0.0), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(pt(x1 + ear, 0.0));
        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
            point1: pt(x1, 0.0),
            point2: pt(x1, ear),
        });
        sink.AddLine(pt(x1, h - r));
        sink.AddArc(&arc(pt(x1 - r, h), r));
        sink.AddLine(pt(x0 + r, h));
        sink.AddArc(&arc(pt(x0, h - r), r));
        sink.AddLine(pt(x0, ear));
        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
            point1: pt(x0, 0.0),
            point2: pt(x0 - ear, 0.0),
        });
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;

        self.target.FillGeometry(&geometry, &self.black, None);
        Ok(())
    }

    unsafe fn draw_eyes(&self, scene: &Scene) {
        let visible = scene.eyes_visible.clamp(0.0, 1.0);
        if visible < 0.01 {
            return;
        }
        // Eyes grow in slightly as they fade in.
        let scale = 0.7 + 0.3 * visible;
        let eye_w = 9.0 * scale;
        let eye_h = (18.0 * scale * (1.0 - 0.85 * scene.blink)).max(2.0);
        let cy = scene.height / 2.0 + 1.0;
        let offset = scene.look * 4.0;

        for side in [-1.0, 1.0] {
            let cx = CANVAS.0 / 2.0 + side * 16.0 * scale + offset;
            // A soft halo first, then the eye itself.
            for (grow, alpha) in [(4.5, 0.05), (2.0, 0.10), (0.0, 1.0)] {
                let (hw, hh) = (eye_w / 2.0 + grow, eye_h / 2.0 + grow);
                let radius = hw.min(hh);
                self.light.SetOpacity(alpha * visible);
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: cx - hw,
                            top: cy - hh,
                            right: cx + hw,
                            bottom: cy + hh,
                        },
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &self.light,
                );
            }
        }
    }

    unsafe fn draw_clock(&self, scene: &Scene) {
        // The clock steps aside quickly so it never overlaps the eyes.
        let alpha = (1.0 - scene.eyes_visible * 1.6).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let cx = CANVAS.0 / 2.0;
        let text: Vec<u16> = scene.clock.encode_utf16().collect();
        self.light.SetOpacity(0.92 * alpha);
        self.target.DrawText(
            &text,
            &self.text,
            &D2D_RECT_F {
                left: cx - scene.width / 2.0,
                top: 0.0,
                right: cx + scene.width / 2.0,
                bottom: scene.height,
            },
            &self.light,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// Quarter-circle corner, drawn clockwise.
fn arc(end: Vector2, radius: f32) -> D2D1_ARC_SEGMENT {
    D2D1_ARC_SEGMENT {
        point: end,
        size: D2D_SIZE_F {
            width: radius,
            height: radius,
        },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    }
}
