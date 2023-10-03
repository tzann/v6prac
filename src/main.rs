use device_query::{DeviceQuery, DeviceState, Keycode};
use femtovg::{renderer::OpenGl, Canvas, Color, ErrorKind, Paint, Path, TextMetrics};
use glutin::config::ConfigTemplateBuilder;
use glutin::context::ContextAttributesBuilder;
use glutin::context::PossiblyCurrentContext;
use glutin::display::GetGlDisplay;
use glutin::prelude::{GlDisplay, NotCurrentGlContextSurfaceAccessor};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, WindowSurface};
use glutin_winit::DisplayBuilder;
use raw_window_handle::*;
use std::num::NonZeroU32;
use std::{collections::VecDeque, time::Instant};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

fn main() {
    let keys_to_track = customize();

    init_and_run(keys_to_track);
}

fn customize() -> Vec<Keycode> {
    let device_state = DeviceState::new();
    // Wait for enter (or any other key) to be released
    loop {
        let poll = device_state.get_keys();
        if poll.is_empty() {
            break;
        }
    }

    println!("Please press all the keys you would like to track, then press Backspace to end customization. If you want to track backspace inputs, tough luck.");
    let mut keys = Vec::new();
    let mut done = false;
    let mut fail_keys = Vec::new();

    while !done {
        let poll = device_state.get_keys();
        for k in poll {
            if k == Keycode::Backspace {
                done = true;
            } else if !keys.contains(&k) {
                if keycode_to_char(&k).is_none() {
                    if !fail_keys.contains(&k) {
                        println!("{:?} not supported (yet). Sorry!", k);
                        fail_keys.push(k)
                    }
                } else {
                    println!("Tracking {:?}", k);
                    keys.push(k);
                }
            }
        }
    }
    println!("{:?} keys recorded.", keys.len());

    keys
}

// How many past polls to display
const MAX_QUEUE_SIZE: usize = 20;
// 1 frame in VVVVVV is 34ms, you will want to adapt this for other games
const NANOS_PER_FRAME: u128 = 34000000;
// How many polls should happen per in-game frame
// Increasing this will make the program more CPU intensive (when limiting fps)
const MAX_POLLS_PER_FRAME: u128 = 20;

struct GlobalState {
    actions: Vec<DisplayableAction>,

    limit_fps: bool,
    last_fps: f64,
    last_dt: u128,

    last_poll_attempt: Instant,
    attempts_since_last_poll: usize,
    poll_queue: VecDeque<RecordedPoll>,

    device_state: DeviceState,

    canvas: Canvas<OpenGl>,
    window: Window,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
}

struct DisplayableAction {
    key: Keycode,
    display_char: DisplayChar,
}

enum DisplayChar {
    Simple(char),
    Rotated(f32, char),
}

struct RecordedPoll {
    timestamp: Instant,
    keys: Vec<bool>,
    dt_before: u128,
    dt_after: u128,
}

pub fn init_and_run(keys_to_track: Vec<Keycode>) {
    let w = 300 + keys_to_track.len() as u32 * 50;
    let h = 800;

    let event_loop = EventLoop::new();
    let (canvas, window, context, surface) = create_windowed_context(&event_loop, w, h);
    let device_state = DeviceState::new();

    let poll_queue = VecDeque::with_capacity(MAX_QUEUE_SIZE);

    let actions = create_actions(keys_to_track);

    let mut state = GlobalState {
        actions,

        limit_fps: false,
        last_fps: 0.0,
        last_dt: 0,

        last_poll_attempt: Instant::now(),
        attempts_since_last_poll: 0,
        poll_queue,

        device_state,

        canvas,
        window,
        context,
        surface,
    };

    event_loop.run(move |event, _, control_flow| {
        event_handler(event, control_flow, &mut state);
    });
}

fn event_handler(event: Event<()>, control_flow: &mut ControlFlow, state: &mut GlobalState) {
    *control_flow = ControlFlow::Poll;

    match event {
        // Last event to be emitted, do any necessary cleanup here
        Event::LoopDestroyed => println!("Goodbye!"),
        // This is polled whenever no events are in queue
        Event::MainEventsCleared => maybe_poll(state),
        // Window isn't resizable, so we only need to deal with CloseRequested
        Event::WindowEvent {
            window_id: _,
            event: WindowEvent::CloseRequested,
        } => *control_flow = ControlFlow::Exit,
        // Render stuff here
        Event::RedrawRequested(_) => render(state),
        _ => (),
    };
}

fn maybe_poll(state: &mut GlobalState) {
    let now = Instant::now();
    let dt = now.duration_since(state.last_poll_attempt);

    // Limit polling rate if needed
    let enough_time_passed = if state.limit_fps {
        NANOS_PER_FRAME <= dt.as_nanos().checked_mul(MAX_POLLS_PER_FRAME).unwrap()
    } else {
        true
    };

    if enough_time_passed {
        state.attempts_since_last_poll += 1;
        state.last_dt = dt.as_nanos();

        poll(&now, state);

        state.last_poll_attempt = now;
    }
}

fn poll(now: &Instant, state: &mut GlobalState) {
    let last_poll = state.poll_queue.front();
    let poll = get_pressed_keys(&state.actions, now, &state.device_state);

    // Record poll if it's the first one, or if the pressed keys have changed
    if last_poll.is_none() || inputs_changed(&poll, last_poll.unwrap()) {
        record_poll(now, poll, state);

        state.window.request_redraw();
    }
}

fn record_poll(now: &Instant, mut poll: RecordedPoll, state: &mut GlobalState) {
    state.last_fps = if let Some(last_poll) = state.poll_queue.front() {
        let delta = now.duration_since(last_poll.timestamp);
        (state.attempts_since_last_poll as f64 / delta.as_secs_f64()).round()
    } else {
        0.0
    };
    state.attempts_since_last_poll = 0;

    if state.poll_queue.len() >= MAX_QUEUE_SIZE {
        state.poll_queue.pop_back();
    }

    poll.dt_before = state.last_dt;
    if let Some(prev_poll) = state.poll_queue.front_mut() {
        prev_poll.dt_after = state.last_dt;
    }

    state.poll_queue.push_front(poll);
}

fn get_pressed_keys(
    actions: &Vec<DisplayableAction>,
    now: &Instant,
    device_state: &DeviceState,
) -> RecordedPoll {
    let poll = device_state.get_keys();

    let mut keys = vec![false; actions.len()];
    for (idx, a) in actions.iter().enumerate() {
        if poll.contains(&a.key) {
            keys[idx] = true;
        }
    }

    RecordedPoll {
        timestamp: *now,
        keys,
        dt_before: u128::MAX,
        dt_after: u128::MAX,
    }
}

fn inputs_changed(this_poll: &RecordedPoll, last_poll: &RecordedPoll) -> bool {
    this_poll.keys.ne(&last_poll.keys)
}

fn render(state: &mut GlobalState) {
    let dpi = state.window.scale_factor();
    let size = state.window.inner_size();

    let bg_color = Color::black();
    let active_paint = Paint::color(Color::rgb(121, 188, 176));
    let inactive_paint = Paint::color(Color::rgb(40, 40, 40));

    state.canvas.set_size(size.width, size.height, dpi as f32);

    state.canvas.clear_rect(
        0,
        0,
        state.canvas.width() as u32,
        state.canvas.height() as u32,
        bg_color,
    );

    draw_fps_counter(
        &mut state.canvas,
        &state.last_fps,
        state.last_dt,
        &active_paint,
    );
    draw_current_inputs(
        &mut state.canvas,
        &state.actions,
        state.poll_queue.front(),
        &active_paint,
        &inactive_paint,
    );
    draw_past_inputs(
        &mut state.canvas,
        &state.actions,
        &state.poll_queue,
        &active_paint,
    );

    state.canvas.flush();

    state.surface.swap_buffers(&state.context).unwrap();
}

fn draw_fps_counter(canvas: &mut Canvas<OpenGl>, fps: &f64, dt: u128, paint: &Paint) {
    let frame_dt = dt as f64 / NANOS_PER_FRAME as f64;

    let _ = canvas.fill_text(
        10.0,
        23.0,
        format!("{: >4} fps +/- {: >2.2}f", fps, frame_dt),
        paint,
    );
}

// TODO: fix magic numbers etc.
fn draw_current_inputs(
    canvas: &mut Canvas<OpenGl>,
    actions: &[DisplayableAction],
    maybe_poll: Option<&RecordedPoll>,
    active_paint: &Paint,
    inactive_paint: &Paint,
) {
    let left_margin = 22.5;
    let text_y_pos = canvas.height() / 2.0 - 22.5;
    let separator_y = canvas.height() / 2.0 - 35.0;

    // Separator line
    let mut path = Path::new();
    path.move_to(0.0, separator_y);
    path.line_to(canvas.width() / 2.0, separator_y);
    canvas.stroke_path(&path, active_paint);

    // Inactive keys
    let mut x = left_margin;
    for action in actions.iter() {
        let _ = match action.display_char {
            DisplayChar::Simple(c) => {
                draw_char_at_pos(canvas, c, x, text_y_pos, 0.0, inactive_paint)
            }
            DisplayChar::Rotated(angle, c) => {
                draw_char_at_pos(canvas, c, x, text_y_pos, angle, inactive_paint)
            }
        };
        x += 25.0;
    }

    if let Some(poll) = maybe_poll {
        // Active keys
        let mut x = left_margin;
        for (idx, &k) in poll.keys.iter().enumerate() {
            if k {
                let action = &actions[idx];
                let _ = match action.display_char {
                    DisplayChar::Simple(c) => {
                        draw_char_at_pos(canvas, c, x, text_y_pos, 0.0, active_paint)
                    }
                    DisplayChar::Rotated(angle, c) => {
                        draw_char_at_pos(canvas, c, x, text_y_pos, angle, active_paint)
                    }
                };
            }
            x += 25.0;
        }
    }
}

fn draw_past_inputs(
    canvas: &mut Canvas<OpenGl>,
    actions: &[DisplayableAction],
    polls: &VecDeque<RecordedPoll>,
    paint: &Paint,
) {
    // No past inputs to render
    if polls.len() <= 1 {
        return;
    }

    let left_margin = 22.5;
    let mut y = canvas.height() / 2.0 - 52.5;

    let mut iter = polls.iter();
    let mut next_poll = iter.next().unwrap();

    for poll in iter {
        // polled inputs could have started up to `dt_before` nanos earlier, ended up to `dt_after` nanos earlier
        // To get the expected duration of the input, we can take `dt - dt_after/2 + dt_before/2`
        // Then we still have an uncertainty of +/- (dt_after + dt_before)/2
        let dt = next_poll.timestamp.duration_since(poll.timestamp);

        let min_nanos_held = dt.as_nanos() - poll.dt_after;
        let epsilon_nanos = (poll.dt_after + poll.dt_before) / 2;

        let frames_held = (min_nanos_held + epsilon_nanos) as f64 / NANOS_PER_FRAME as f64;
        let _uncertainty = epsilon_nanos as f64 / NANOS_PER_FRAME as f64;

        let mut x = left_margin;
        for (idx, &k) in poll.keys.iter().enumerate() {
            if k {
                let action = &actions[idx];
                let _ = match action.display_char {
                    DisplayChar::Simple(c) => draw_char_at_pos(canvas, c, x, y, 0.0, paint),
                    DisplayChar::Rotated(angle, c) => {
                        draw_char_at_pos(canvas, c, x, y, angle, paint)
                    }
                };
            }
            x += 25.0;
        }
        let frame_text = format!("{: >8.2} f", frames_held);
        let _ = draw_text_at_pos(canvas, frame_text, x, y, paint);

        next_poll = poll;
        y -= 20.0;

        // Stop rendering if too many polls in queue
        if y <= 40.0 {
            break;
        }
    }
}

fn draw_char_at_pos(
    canvas: &mut Canvas<OpenGl>,
    c: char,
    x: f32,
    y: f32,
    angle: f32,
    paint: &Paint,
) -> Result<TextMetrics, ErrorKind> {
    let text = c.to_string();
    let metrics = canvas.measure_text(x, y, &text, paint).unwrap();
    let h = metrics.height();
    let w = metrics.width();

    canvas.save();

    if angle == 0.0 || angle == -0.0 {
        // Move to bottom left corner of character
        canvas.translate(x.round(), (y + h).round());
    } else {
        // Move to center of character
        canvas.translate((x + w / 2.0).round(), (y + h / 2.0).round());
        // Rotate around center
        canvas.rotate(angle);
        // Move to "origin" of character (bottom left)
        canvas.translate((w / -2.0).round(), (h / 2.0).round());
    }
    // Draw character
    let res = canvas.fill_text(0.0, 0.0, &text, paint);

    // Restore canvas
    canvas.restore();

    res
}

fn draw_text_at_pos<S>(
    canvas: &mut Canvas<OpenGl>,
    text: S,
    x: f32,
    y: f32,
    paint: &Paint,
) -> Result<TextMetrics, ErrorKind>
where
    S: AsRef<str>,
{
    let metrics = canvas.measure_text(x, y, &text, paint).unwrap();
    let h = metrics.height();

    canvas.save();

    // Move to bottom left corner of text
    canvas.translate(x.round(), (y + h).round());
    // Draw text
    let res = canvas.fill_text(0.0, 0.0, &text, paint);

    // Restore canvas
    canvas.restore();

    res
}

fn create_windowed_context<T>(
    event_loop: &EventLoop<T>,
    w: u32,
    h: u32,
) -> (
    Canvas<OpenGl>,
    Window,
    PossiblyCurrentContext,
    Surface<WindowSurface>,
) {
    let window_size = winit::dpi::PhysicalSize::new(w, h);
    let window_builder = WindowBuilder::new()
        .with_title("v6prac")
        .with_inner_size(window_size)
        .with_resizable(false);

    let template = ConfigTemplateBuilder::new().with_alpha_size(8);

    let display_builder = DisplayBuilder::new().with_window_builder(Some(window_builder));

    let (window, gl_config) = display_builder
        .build(event_loop, template, |mut configs| configs.next().unwrap())
        .unwrap();

    let window = window.unwrap();

    let raw_window_handle = Some(window.raw_window_handle());

    let gl_display = gl_config.display();

    let context_attributes = ContextAttributesBuilder::new().build(raw_window_handle);
    let fallback_context_attributes = ContextAttributesBuilder::new()
        .with_context_api(glutin::context::ContextApi::Gles(None))
        .build(raw_window_handle);
    let mut not_current_gl_context = Some(unsafe {
        gl_display
            .create_context(&gl_config, &context_attributes)
            .unwrap_or_else(|_| {
                gl_display
                    .create_context(&gl_config, &fallback_context_attributes)
                    .expect("failed to create context")
            })
    });

    let (width, height): (u32, u32) = window.inner_size().into();
    let raw_window_handle = window.raw_window_handle();
    let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
        raw_window_handle,
        NonZeroU32::new(width).unwrap(),
        NonZeroU32::new(height).unwrap(),
    );

    let surface = unsafe {
        gl_config
            .display()
            .create_window_surface(&gl_config, &attrs)
            .unwrap()
    };

    let gl_context = not_current_gl_context
        .take()
        .unwrap()
        .make_current(&surface)
        .unwrap();

    let renderer =
        unsafe { OpenGl::new_from_function_cstr(|s| gl_display.get_proc_address(s) as *const _) }
            .expect("Cannot create renderer");

    let mut canvas = Canvas::new(renderer).expect("Cannot create canvas!");

    canvas.set_size(w, h, window.scale_factor() as f32);
    canvas.scale(2.0, 2.0);

    let _font_id = canvas
        .add_font("Commodore Pixelized v1.2.ttf")
        .expect("Couldn't add font. Is it in the right folder?");

    (canvas, window, gl_context, surface)
}

fn create_actions(keys_to_track: Vec<Keycode>) -> Vec<DisplayableAction> {
    let mut actions = Vec::with_capacity(keys_to_track.len());
    for k in keys_to_track.iter() {
        let angle = match k {
            Keycode::Up => 0.0,
            Keycode::Down => std::f32::consts::PI,
            Keycode::Left => -std::f32::consts::FRAC_PI_2,
            Keycode::Right => std::f32::consts::FRAC_PI_2,
            _ => 0.0,
        };
        let c = keycode_to_char(k).unwrap();

        let display_char = if angle != 0.0 {
            DisplayChar::Rotated(angle, c)
        } else {
            DisplayChar::Simple(c)
        };

        actions.push(DisplayableAction {
            key: *k,
            display_char,
        });
    }

    actions
}

fn keycode_to_char(keycode: &Keycode) -> Option<char> {
    match keycode {
        Keycode::Key0 => Some('0'),
        Keycode::Key1 => Some('1'),
        Keycode::Key2 => Some('2'),
        Keycode::Key3 => Some('3'),
        Keycode::Key4 => Some('4'),
        Keycode::Key5 => Some('5'),
        Keycode::Key6 => Some('6'),
        Keycode::Key7 => Some('7'),
        Keycode::Key8 => Some('8'),
        Keycode::Key9 => Some('9'),

        Keycode::A => Some('A'),
        Keycode::B => Some('B'),
        Keycode::C => Some('C'),
        Keycode::D => Some('D'),
        Keycode::E => Some('E'),
        Keycode::F => Some('F'),
        Keycode::G => Some('G'),
        Keycode::H => Some('H'),
        Keycode::I => Some('I'),
        Keycode::J => Some('J'),
        Keycode::K => Some('K'),
        Keycode::L => Some('L'),
        Keycode::M => Some('M'),
        Keycode::N => Some('N'),
        Keycode::O => Some('O'),
        Keycode::P => Some('P'),
        Keycode::Q => Some('Q'),
        Keycode::R => Some('R'),
        Keycode::S => Some('S'),
        Keycode::T => Some('T'),
        Keycode::U => Some('U'),
        Keycode::V => Some('V'),
        Keycode::W => Some('W'),
        Keycode::X => Some('X'),
        Keycode::Y => Some('Y'),
        Keycode::Z => Some('Z'),

        Keycode::Up => Some('^'),
        Keycode::Down => Some('^'),
        Keycode::Left => Some('^'),
        Keycode::Right => Some('^'),

        Keycode::Numpad0 => Some('0'),
        Keycode::Numpad1 => Some('1'),
        Keycode::Numpad2 => Some('2'),
        Keycode::Numpad3 => Some('3'),
        Keycode::Numpad4 => Some('4'),
        Keycode::Numpad5 => Some('5'),
        Keycode::Numpad6 => Some('6'),
        Keycode::Numpad7 => Some('7'),
        Keycode::Numpad8 => Some('8'),
        Keycode::Numpad9 => Some('9'),
        Keycode::NumpadSubtract => Some('-'),
        Keycode::NumpadAdd => Some('+'),
        Keycode::NumpadDivide => Some('/'),
        Keycode::NumpadMultiply => Some('*'),
        Keycode::Grave => Some('`'),
        Keycode::Minus => Some('-'),
        Keycode::Equal => Some('='),
        Keycode::LeftBracket => Some('['),
        Keycode::RightBracket => Some(']'),
        Keycode::BackSlash => Some('\\'),
        Keycode::Semicolon => Some(':'),
        Keycode::Apostrophe => Some('\''),
        Keycode::Comma => Some(','),
        Keycode::Dot => Some('.'),
        Keycode::Slash => Some('/'),

        _ => None,
    }
}
