use std::{collections::VecDeque, time::Instant};
use device_query::{DeviceState, Keycode, DeviceQuery};
use femtovg::{renderer::OpenGl, Canvas, Color, Paint, Path};
use glutin::{event_loop::{EventLoop, ControlFlow}, window::{WindowBuilder, Window}, ContextBuilder, ContextWrapper, PossiblyCurrent, event::{Event, WindowEvent}};

fn main() {
    println!("Would you like to limit fps? y/n");
    let mut buffer = String::new();
    std::io::stdin().read_line(&mut buffer).unwrap();

    init_and_run(buffer.contains('y'));
}

// How many past polls to display
const MAX_QUEUE_SIZE: usize = 30;
// 1 frame in VVVVVV is 34ms, you will want to adapt this for other games
const NANOS_PER_FRAME: u128 = 34000000;
// How many polls should happen per in-game frame
// Increasing this will make the program more CPU intensive (when limiting fps)
const MAX_POLLS_PER_FRAME: u128 = 10;

// Window dimensions
const WINDOW_WIDTH: u32 = 300;
const WINDOW_HEIGHT: u32 = 800;

// Order matters here (for displaying purposes)
const NUM_ACTIONS: usize = 6;
const ACTIONS: [DisplayableAction; NUM_ACTIONS] = [
    DisplayableAction { key: Keycode::E, display_char: "E" },
    DisplayableAction { key: Keycode::R, display_char: "R" },
    DisplayableAction { key: Keycode::V, display_char: "V" },
    DisplayableAction { key: Keycode::Z, display_char: "Z" },
    DisplayableAction { key: Keycode::Left, display_char: "<" },
    DisplayableAction { key: Keycode::Right, display_char: ">" },
];

struct GlobalState {
    limit_fps: bool,
    last_fps: f64,

    last_poll_attempt: Instant,
    attempts_since_last_poll: usize,
    poll_queue: VecDeque<RecordedPoll>,

    device_state: DeviceState,
    windowed_context: ContextWrapper<PossiblyCurrent, Window>,
    canvas: Canvas<OpenGl>,
}

struct DisplayableAction<'a> {
    key: Keycode,
    display_char: &'a str,
}

struct RecordedPoll {
    timestamp: Instant,
    keys: [bool; NUM_ACTIONS],
}

pub fn init_and_run(limit_fps: bool) {
    let event_loop = EventLoop::new();
    let windowed_context = create_windowed_context(&event_loop);
    let canvas = create_canvas(&windowed_context);
    let device_state = DeviceState::new();

    let poll_queue = VecDeque::with_capacity(MAX_QUEUE_SIZE);

    let mut state = GlobalState {
        limit_fps,
        last_fps: 0.0,

        last_poll_attempt: Instant::now(),
        attempts_since_last_poll: 0,
        poll_queue,

        device_state,
        windowed_context,
        canvas,
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
        Event::WindowEvent { window_id: _, event } => match event {
            WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
            _ => (),
        },
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

        poll(&now, state);

        state.last_poll_attempt = now;
    }
}

fn poll(now: &Instant, state: &mut GlobalState) {
    let last_poll = state.poll_queue.front();
    let poll = get_pressed_keys(&now, &state.device_state);

    // Record poll if it's the first one, or if the pressed keys have changed
    if last_poll.is_none() || inputs_changed(&poll, last_poll.unwrap()) {
        record_poll(&now, poll, state);
        
        state.windowed_context.window().request_redraw();
    }
}

fn record_poll(now: &Instant, poll: RecordedPoll, state: &mut GlobalState) {
    state.last_fps = if let Some(last_poll) = state.poll_queue.front() {
        let dt = now.duration_since(last_poll.timestamp).as_secs_f64();
        (state.attempts_since_last_poll as f64 / dt).round()
    } else {
        0.0
    };
    state.attempts_since_last_poll = 0;

    if state.poll_queue.len() >= MAX_QUEUE_SIZE {
        state.poll_queue.pop_back();
    }
    state.poll_queue.push_front(poll);
}

fn get_pressed_keys(now: &Instant, device_state: &DeviceState) -> RecordedPoll {
    let poll = device_state.get_keys();

    let mut keys = [false; NUM_ACTIONS];
    for (idx, a) in ACTIONS.iter().enumerate() {
        if poll.contains(&a.key) {
            keys[idx] = true;
        }
    }

    RecordedPoll {
        timestamp: now.clone(),
        keys,
    }
}

fn inputs_changed(this_poll: &RecordedPoll, last_poll: &RecordedPoll) -> bool {
    this_poll.keys.ne(&last_poll.keys)
}

fn render(state: &mut GlobalState) {
    let dpi = state.windowed_context.window().scale_factor();
    let size = state.windowed_context.window().inner_size();
    
    let bg_color = Color::black();
    let active_paint = Paint::color(Color::rgb(121, 188, 176));
    let inactive_paint = Paint::color(Color::rgb(40, 40, 40));

    state.canvas.set_size(size.width, size.height, dpi as f32);

    state.canvas.clear_rect(0, 0, state.canvas.width() as u32, state.canvas.height() as u32, bg_color);

    draw_fps_counter(&mut state.canvas, &state.last_fps, active_paint);
    draw_current_inputs(&mut state.canvas, state.poll_queue.front(), active_paint, inactive_paint);
    draw_past_inputs(&mut state.canvas, &state.poll_queue, active_paint);

    state.canvas.flush();
    state.windowed_context.swap_buffers().unwrap();
}

fn draw_fps_counter(canvas: &mut Canvas<OpenGl>, fps: &f64, paint: Paint) {
    let _ = canvas.fill_text(
        10.0, 
        23.0,
        format!("{: >4} fps", fps),
        paint,
    );
}

fn draw_current_inputs(canvas: &mut Canvas<OpenGl>, maybe_poll: Option<&RecordedPoll>, active_paint: Paint, inactive_paint: Paint) {
    let left_margin = 10.0;
    let text_y_pos = canvas.height() - 10.0;
    let separator_y = canvas.height() - 35.0;
    
    // Separator line
    let mut path = Path::new();
    path.move_to(0.0, separator_y);
    path.line_to(canvas.width(), separator_y);
    canvas.stroke_path(&mut path, active_paint);

    // Inactive keys
    let all_keys = get_key_string(&[true; NUM_ACTIONS]);
    let _ = canvas.fill_text(left_margin, text_y_pos, all_keys, inactive_paint);

    if let Some(poll) = maybe_poll {
        // Active keys
        let pressed_keys = get_key_string(&poll.keys);
        let _ = canvas.fill_text(left_margin, text_y_pos, pressed_keys, active_paint);
    }
}

fn draw_past_inputs(canvas: &mut Canvas<OpenGl>, polls: &VecDeque<RecordedPoll>, paint: Paint) {
    // No past inputs to render
    if polls.len() <= 1 {
        return;
    }
    
    let left_margin = 10.0;
    let mut y = canvas.height() as f32 - 40.0;

    let mut iter = polls.iter();
    let mut next_poll = iter.next().unwrap();

    for poll in iter {
        let dt = next_poll.timestamp.duration_since(poll.timestamp);
        let frames_held = dt.as_nanos() as f64 / NANOS_PER_FRAME as f64;

        let poll_text = format!("{} {: >8.2} f", get_key_string(&poll.keys), frames_held);
        let _ = canvas.fill_text(left_margin, y, poll_text, paint);

        next_poll = poll;
        y -= 20.0;

        // Stop rendering if too many polls in queue
        if y <= 40.0 {
            break;
        }
    }
}

fn get_key_string(keys: &[bool; NUM_ACTIONS]) -> String {
    let mut res = "".to_string();

    for (idx, display_action) in ACTIONS.iter().enumerate() {
        if keys[idx] {
            res = res + " " + display_action.display_char;
        } else {
            res = res + "  ";
        }
    }

    res
}

fn create_windowed_context<T>(event_loop: &EventLoop<T>) -> ContextWrapper<PossiblyCurrent, Window> {
    let window_size = glutin::dpi::PhysicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT);
    let window_builder = WindowBuilder::new()
        .with_title("tzann's input display")
        .with_inner_size(window_size)
        .with_resizable(false);
    
    let windowed_context = ContextBuilder::new()
        .build_windowed(window_builder, &event_loop)
        .unwrap();
    
    unsafe { windowed_context.make_current().unwrap() }
}

fn create_canvas(windowed_context: &ContextWrapper<PossiblyCurrent, Window>) -> Canvas<OpenGl> {
    let renderer = OpenGl::new_from_glutin_context(&windowed_context).expect("Cannot create renderer");
    let dpi = windowed_context.window().scale_factor();
    
    let mut canvas = Canvas::new(renderer)
        .expect("Cannot create canvas!");
    canvas.set_size(WINDOW_WIDTH, WINDOW_HEIGHT, dpi as f32);

    let _font_id = canvas.add_font("Commodore Pixelized v1.2.ttf")
        .expect("Couldn't add font. Is it in the right folder?");

    canvas
}