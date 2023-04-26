#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use crossbeam_channel::Sender;
use eframe::egui::{self, ComboBox, Slider, Ui};
use groove_core::{
    generators::Waveform,
    time::ClockNano,
    traits::{Performs, Resets},
    FrequencyHz, ParameterType, StereoSample, SAMPLE_BUFFER_SIZE,
};
use groove_entities::{controllers::LfoController, instruments::WelshSynth};
use groove_orchestration::Orchestrator;
use groove_settings::SongSettings;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};
use stream::{AudioInterfaceInput, AudioQueue, AudioStreamService};
use strum::IntoEnumIterator;

mod stream;

fn main() -> Result<(), eframe::Error> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).
    let options = eframe::NativeOptions {
        initial_window_size: Some(egui::vec2(1920.0, 1080.0)),
        ..Default::default()
    };
    eframe::run_native(
        "Audio Prototype (egui)",
        options,
        Box::new(|_cc| Box::<AudioPrototype2>::default()),
    )
}

struct AudioPrototype2 {
    orchestrator: Arc<Mutex<Orchestrator>>,
    name: String,
    bpm: ParameterType,
    sample_rate: Arc<Mutex<usize>>,

    audio_stream_sender: Sender<AudioInterfaceInput>,
    control_bar: ControlBar,
}
impl Default for AudioPrototype2 {
    fn default() -> Self {
        let clock_settings = ClockNano::default();
        let audio_stream_service = AudioStreamService::new();
        let audio_stream_sender = audio_stream_service.sender().clone();
        let orchestrator = Arc::new(Mutex::new(Orchestrator::new_with(clock_settings)));
        let orchestrator_clone = Arc::clone(&orchestrator);
        let sample_rate = Arc::new(Mutex::new(0));
        Self::start_audio_stream(
            orchestrator_clone,
            audio_stream_service,
            Arc::clone(&sample_rate),
        );
        Self {
            bpm: Default::default(),
            orchestrator,
            name: "Arthur".to_owned(),

            sample_rate,
            audio_stream_sender,
            control_bar: Default::default(),
        }
    }
}

impl eframe::App for AudioPrototype2 {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(o) = self.orchestrator.lock() {
            self.bpm = o.bpm();
        }
        if let Ok(mut o) = self.orchestrator.lock() {
            egui::TopBottomPanel::top("control-strip")
                .show(ctx, |ui| self.control_bar.show(ui, &mut o));
            egui::TopBottomPanel::bottom("orchestrator").show(ctx, |ui| o.show(ui));
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Audio Prototype 2");
            ui.horizontal(|ui| {
                let name_label = ui.label("Your name: ");
                ui.text_edit_singleline(&mut self.name)
                    .labelled_by(name_label.id);
            });
            ui.add(egui::Slider::new(&mut self.bpm, 1.0..=999.9).text("BPM"));
            if ui.button("Increase BPM").clicked() {
                self.bpm += 1.0;
            }
            if ui.button("load").clicked() {
                let filename =
                    "/home/miket/src/groove/projects/demos/controllers/stereo-automation.yaml";
                match SongSettings::new_from_yaml_file(filename) {
                    Ok(s) => {
                        let pb = PathBuf::from("/home/miket/src/groove/assets");
                        match s.instantiate(&pb, false) {
                            Ok(instance) => {
                                if let Ok(mut o) = self.orchestrator.lock() {
                                    if let Ok(sample_rate) = self.sample_rate.lock() {
                                        *o = instance;
                                        self.bpm = o.bpm();
                                        o.reset(*sample_rate);
                                    }
                                }
                            }
                            Err(err) => eprintln!("instantiate: {}", err),
                        }
                    }
                    Err(err) => eprintln!("new_from_yaml: {}", err),
                }
            }
            if let Ok(o) = self.orchestrator.lock() {
                ui.label(format!("clock: {:?}", o.clock()));
            }
            ui.label(format!("Hello '{}', BPM {}", self.name, self.bpm));
        });
        if let Ok(mut o) = self.orchestrator.lock() {
            if self.bpm != o.bpm() {
                o.set_bpm(self.bpm);
                eprintln!("BPM is now {}", self.bpm)
            }
        }
    }
}
impl AudioPrototype2 {
    fn start_audio_stream(
        orchestrator_clone: Arc<Mutex<Orchestrator>>,
        audio_stream_service: AudioStreamService,
        sample_rate_clone: Arc<Mutex<usize>>,
    ) {
        std::thread::spawn(move || {
            let orchestrator = orchestrator_clone;
            let mut queue_opt = None;
            loop {
                if let Ok(event) = audio_stream_service.receiver().recv() {
                    match event {
                        stream::AudioInterfaceEvent::Reset(sample_rate, queue) => {
                            if let Ok(mut sr) = sample_rate_clone.lock() {
                                *sr = sample_rate;
                            }
                            if let Ok(mut o) = orchestrator.lock() {
                                o.reset(sample_rate);
                            }
                            queue_opt = Some(queue);
                            eprintln!("got a queue");
                        }
                        stream::AudioInterfaceEvent::NeedsAudio(_when, count) => {
                            if let Some(queue) = queue_opt.as_ref() {
                                if let Ok(o) = orchestrator.lock() {
                                    Self::generate_audio(o, queue, (count / 64) as u8);
                                }
                            }
                        }
                        stream::AudioInterfaceEvent::Quit => todo!(),
                    }
                }
            }
        });
    }

    fn generate_audio(
        mut orchestrator: MutexGuard<Orchestrator>,
        queue: &AudioQueue,
        buffer_count: u8,
    ) {
        let mut samples = [StereoSample::SILENCE; SAMPLE_BUFFER_SIZE];
        for i in 0..buffer_count {
            let is_last_iteration = i == buffer_count - 1;

            let (response, ticks_completed) = orchestrator.tick(&mut samples);
            if ticks_completed < samples.len() {
                // self.stop_playback();
                // self.reached_end_of_playback = true;
            }

            for sample in samples {
                let _ = queue.push(sample);
            }

            match response.0 {
                groove_orchestration::messages::Internal::None => {}
                groove_orchestration::messages::Internal::Single(event) => {
                    //                    self.handle_groove_event(event);
                }
                groove_orchestration::messages::Internal::Batch(events) => {
                    for event in events {
                        //                      self.handle_groove_event(event)
                    }
                }
            }
            // if is_last_iteration {
            //     // This clock is used to tell the app where we are in the song, so
            //     // even though it looks like it's not helping here in the loop, it's
            //     // necessary.
            //     self.update_control_bar_clock();
            // }
        }
    }
}

#[derive(Debug, Default)]
struct ControlBar {}
impl ControlBar {
    fn show(&self, ui: &mut egui::Ui, orchestrator: &mut Orchestrator) {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
            if ui.button("start over").clicked() {
                orchestrator.skip_to_start();
            }
            if ui.button("play").clicked() {
                orchestrator.play();
            }
            if ui.button("pause").clicked() {
                orchestrator.stop();
            }
        });
    }
}

trait Shows {
    fn show(&mut self, ui: &mut egui::Ui);
}

impl Shows for WelshSynth {
    fn show(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("hello! {}", self.gain().value()));
    }
}

impl Shows for Orchestrator {
    fn show(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
            let mut id = 0;

            let uids: Vec<usize> = self.entity_iter().map(|(uid, _entity)| *uid).collect();
            for uid in uids {
                let entity = self.get_mut(uid).unwrap();
                egui::Frame::none()
                    .fill(egui::Color32::DARK_GRAY)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.heading(entity.as_has_uid().name());
                            match entity {
                                groove_orchestration::Entity::Arpeggiator(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterAllPass(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterBandPass(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterBandStop(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterHighPass(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterHighShelf(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterLowPass12db(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterLowPass24db(e) => {
                                    let mut cutoff = e.cutoff().value();
                                    let mut pbr = e.passband_ripple();
                                    if ui
                                        .add(
                                            Slider::new(&mut cutoff, FrequencyHz::range())
                                                .text("Cutoff"),
                                        )
                                        .changed()
                                    {
                                        e.set_cutoff(cutoff.into());
                                    };
                                    if ui
                                        .add(Slider::new(&mut pbr, 0.0..=10.0).text("Passband"))
                                        .changed()
                                    {
                                        e.set_passband_ripple(pbr)
                                    };
                                }
                                groove_orchestration::Entity::BiQuadFilterLowShelf(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterNone(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::BiQuadFilterPeakingEq(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Bitcrusher(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Chorus(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Clock(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Compressor(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::ControlTrip(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::DebugSynth(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Delay(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Drumkit(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::FmSynth(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Gain(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::LfoController(e) => {
                                    let mut frequency = e.frequency().value();
                                    let mut waveform = e.waveform();
                                    if ui
                                        .add(
                                            Slider::new(
                                                &mut frequency,
                                                LfoController::frequency_range(),
                                            )
                                            .text("Frequency"),
                                        )
                                        .changed()
                                    {
                                        e.set_frequency(frequency.into());
                                    };
                                    ComboBox::new(ui.next_auto_id(), "Waveform")
                                        .selected_text(waveform.to_string())
                                        .show_ui(ui, |ui| {
                                            for w in Waveform::iter() {
                                                ui.selectable_value(
                                                    &mut waveform,
                                                    w,
                                                    w.to_string(),
                                                );
                                            }
                                        });
                                    if waveform != e.waveform() {
                                        eprintln!("changed {} {}", e.waveform(), waveform);
                                        e.set_waveform(waveform);
                                    }
                                }
                                groove_orchestration::Entity::Limiter(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::MidiTickSequencer(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Mixer(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::PatternManager(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Reverb(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Sampler(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Sequencer(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::SignalPassthroughController(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Timer(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::ToyAudioSource(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::ToyController(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::ToyEffect(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::ToyInstrument(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::ToySynth(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::Trigger(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                                groove_orchestration::Entity::WelshSynth(e) => {
                                    ui.label(entity.as_has_uid().name());
                                }
                            }
                        })
                    });
                id += 1;
            }
        });
    }
}
