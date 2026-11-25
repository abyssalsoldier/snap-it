//! Windows.Graphics.Capture for one monitor, cropped on the GPU to a region.

use snapit_gfx::Rect;
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

pub struct Capture {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    staging: ID3D11Texture2D,
    region: Rect,
    pixels: Vec<u8>,
}

fn e(err: windows::core::Error) -> String {
    let m = err.message().to_string();
    if m.is_empty() { format!("{err:?}") } else { m }
}

impl Capture {
    pub fn new(monitor: isize, region: Rect, cursor: bool) -> Result<Capture, String> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err("screen recording needs Windows 10 version 1903 or later".into());
        }
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).map_err(e)?;
            let device: ID3D11Device = device.ok_or("no D3D11 device")?;
            let context: ID3D11DeviceContext = context.ok_or("no D3D11 context")?;
            let dxgi: IDXGIDevice = device.cast().map_err(e)?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(e)?.cast().map_err(e)?;

            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(e)?;
            let item: GraphicsCaptureItem = interop.CreateForMonitor(HMONITOR(monitor as *mut _)).map_err(e)?;
            let size = item.Size().map_err(e)?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(e)?;
            let session = pool.CreateCaptureSession(&item).map_err(e)?;
            let _ = session.SetIsCursorCaptureEnabled(cursor); // Windows 10 2004+
            session.StartCapture().map_err(e)?;

            let desc = D3D11_TEXTURE2D_DESC {
                Width: region.w as u32,
                Height: region.h as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(e)?;
            let staging = staging.ok_or("no staging texture")?;
            Ok(Capture { _device: device, context, pool, session, staging, region, pixels: vec![0; (region.w * region.h * 4) as usize] })
        }
    }

    /// Copies the newest available frame into `pixels`. Returns false if none arrived.
    pub fn latest(&mut self) -> Result<bool, String> {
        let mut newest = None;
        while let Ok(f) = self.pool.TryGetNextFrame() {
            newest = Some(f);
        }
        let Some(frame) = newest else { return Ok(false) };
        unsafe {
            let surface = frame.Surface().map_err(e)?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(e)?;
            let tex: ID3D11Texture2D = access.GetInterface().map_err(e)?;
            let mut d = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut d);
            let r = self.region;
            if (r.right() as u32) > d.Width || (r.bottom() as u32) > d.Height {
                return Err("the monitor's resolution changed during recording".into());
            }
            let bx = D3D11_BOX { left: r.x as u32, top: r.y as u32, front: 0, right: r.right() as u32, bottom: r.bottom() as u32, back: 1 };
            let dst: ID3D11Resource = self.staging.cast().map_err(e)?;
            let src: ID3D11Resource = tex.cast().map_err(e)?;
            self.context.CopySubresourceRegion(&dst, 0, 0, 0, 0, &src, 0, Some(&bx));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&dst, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(e)?;
            let row = r.w as usize * 4;
            for y in 0..r.h as usize {
                let src = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), row);
                self.pixels[y * row..(y + 1) * row].copy_from_slice(src);
            }
            self.context.Unmap(&dst, 0);
        }
        let _ = frame.Close();
        Ok(true)
    }

    /// The most recent frame, BGRA, top-down, `region.w * 4` bytes per row.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
