use wayland_client::{Connection, Dispatch, QueueHandle as Queue, Proxy, protocol::{
	wl_registry::{self as registry, WlRegistry as Registry},
	wl_compositor::{self as compositor, WlCompositor as Compositor},
	wl_output::{self as output, WlOutput as Output},
	wl_surface::{self as surface, WlSurface as Surface},
}};
use wayland_protocols::xdg::shell::client::{
	xdg_wm_base::{self as wm_base, XdgWmBase as WmBase},
	xdg_surface::{self, XdgSurface},
	xdg_toplevel::{self as toplevel, XdgToplevel as TopLevel}
};

const SIZE: (usize, usize) = (3840, 360);

pub struct State {
    wm_base: Option<WmBase>,
	scale: u32,
	surface: Option<Surface>,
	xdg_surface: Option<(XdgSurface, TopLevel)>,
	width: usize, height: usize
}

impl Dispatch<Registry, ()> for State {
    fn event(&mut self, registry: &Registry, event: registry::Event, _: &(), _: &Connection, queue: &Queue<Self>) {
		match event {
			registry::Event::Global{name, interface, version, ..} => match &interface[..] {
				"wl_compositor" => {
					let compositor = registry.bind::<Compositor, _, _>(name, version, queue, ()).unwrap();
					self.surface = Some(compositor.create_surface(queue, ()).unwrap());
					self.surface.as_ref().unwrap().set_buffer_scale(self.scale as _);
				},
				"wl_output" => { registry.bind::<Output, _, _>(name, version, queue, ()).unwrap(); }
				"xdg_wm_base" => self.wm_base = Some(registry.bind::<WmBase, _, _>(name, version, queue, ()).unwrap()),
				_ => {}
			},
			_ => {}
		};
	}
}

impl Dispatch<Compositor, ()> for State {
    fn event(&mut self, _: &Compositor, _: compositor::Event, _: &(), _: &Connection, _: &Queue<Self>) {}
}

impl Dispatch<Output, ()> for State {
    fn event(&mut self, _: &Output, event: output::Event, _: &(), _: &Connection, _: &Queue<Self>) {
		println!("Output");
		match event {
			output::Event::Scale{factor} => {
				self.scale = factor as _;
				self.surface.as_ref().unwrap().set_buffer_scale(self.scale as _);
			}
			_ => {}
		}
	}
}

impl Dispatch<Surface, ()> for State {
    fn event(&mut self, _: &Surface, _: surface::Event, _: &(), _: &Connection, _: &Queue<Self>) {}
}

impl Dispatch<WmBase, ()> for State {
    fn event(&mut self, wm_base: &WmBase, event: wm_base::Event, _: &(), _: &Connection, _: &Queue<Self>) {
		if let wm_base::Event::Ping{serial} = event { wm_base.pong(serial); } else { unreachable!() };
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(&mut self, xdg_surface: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &Queue<Self>) {
		println!("XDG Surface");
		if let xdg_surface::Event::Configure{serial} = event {
			xdg_surface.ack_configure(serial);
			(self.width, self.height) = SIZE;
		} else { unreachable!() }
    }
}

impl Dispatch<TopLevel, ()> for State {
    fn event(&mut self, _: &TopLevel, event: toplevel::Event, _: &(), _: &Connection, _: &Queue<Self>) {
		match event {
        	toplevel::Event::Close => self.xdg_surface = None,
			_ => println!("{event:?}")
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let mut event_queue = connection.new_event_queue();
    let ref queue = event_queue.handle();
	let display = connection.display();
    display.get_registry(queue, ())?;

	let ref mut state = State{
		wm_base: None,
		scale: 3,
		surface: None,
		xdg_surface: None,
        width: 0, height: 0,
	};

	println!("Wait for surface");
    while state.wm_base.is_none() || state.surface.is_none() { event_queue.blocking_dispatch(state)?; }
	let (wm_base, surface) = (state.wm_base.as_ref().unwrap(), state.surface.as_ref().unwrap());
	let xdg_surface = wm_base.get_xdg_surface(surface, queue, ()).unwrap();
	let toplevel = xdg_surface.get_toplevel(queue, ()).unwrap();
	toplevel.set_title("Piet GPU".into());
	surface.commit();
	state.xdg_surface = Some((xdg_surface, toplevel));
	//println!("Wait for configure");
	//while (state.width, state.height) == (0, 0) { event_queue.blocking_dispatch(state)?; }
	(state.width, state.height) = SIZE;

    struct RawWindowHandle(raw_window_handle::RawWindowHandle);
    unsafe impl raw_window_handle::HasRawWindowHandle for RawWindowHandle { fn raw_window_handle(&self) -> raw_window_handle::RawWindowHandle { self.0 } }
    let (instance, gpu_surface) = piet_gpu_hal::Instance::new(Some(&RawWindowHandle(raw_window_handle::RawWindowHandle::Wayland({
        let mut s=raw_window_handle::WaylandHandle::empty();
        s.display = display.id().as_ptr() as *mut _;
        s.surface = state.surface.as_ref().unwrap().id().as_ptr() as  *mut _;
        s
    }))), Default::default())?;
    let device = unsafe{instance.device(gpu_surface.as_ref())}?;
    let (width, height) = (state.width, state.height);
    let gpu_surface = gpu_surface.unwrap();
    let mut swapchain = unsafe{instance.swapchain(width, height, &device, &gpu_surface)}?;
    let session = piet_gpu_hal::Session::new(device);
    let present_semaphore = unsafe{session.create_semaphore()}?;
    let mut renderer = unsafe{piet_gpu::Renderer::new(&session, width, height, 1)}?;
    let mut cx = piet_gpu::PietGpuRenderContext::new();
    use piet::{RenderContext, Text, TextLayoutBuilder};
    let layout = cx.text().new_text_layout("Hello World!").default_attribute(piet::TextAttribute::FontSize(height as _)).build().unwrap();
    cx.draw_text(&layout, piet::kurbo::Point{x: 0., y: height as _});
    renderer.upload_render_ctx(&mut cx, 0)?;
	let (image_idx, acquisition_semaphore) = unsafe{swapchain.next()}?;
    let image = unsafe{swapchain.image(image_idx)};
	let ref query_pool = session.create_query_pool(12)?;
    let mut cmd_buf = session.cmd_buf()?;
    unsafe{
        cmd_buf.begin();
        renderer.record(&mut cmd_buf, &query_pool, 0);
        use piet_gpu_hal::ImageLayout;
        cmd_buf.image_barrier(&image, ImageLayout::Undefined, ImageLayout::BlitDst);
        cmd_buf.blit_image(&renderer.image_dev, &image);
        cmd_buf.image_barrier(&image, ImageLayout::BlitDst, ImageLayout::Present);
        cmd_buf.finish();
        let submitted = session.run_cmd_buf(cmd_buf, &[&acquisition_semaphore], &[&present_semaphore])?;
        swapchain.present(image_idx, &[&present_semaphore])?;
        submitted.wait()?;
    }

	println!("Wait for close");
	while state.xdg_surface.is_some() { event_queue.blocking_dispatch(state)?; }
    Ok(())
}
