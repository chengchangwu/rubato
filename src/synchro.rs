use crate::windows::WindowFunction;

use crate::sinc::make_sincs;
use num_traits::Float;
use num::integer;
use std::error;
use std::fmt;

//use rustfft::algorithm::Radix4;
use rustfft::FFTplanner;
use rustfft::num_complex::Complex;
use rustfft::num_traits::Zero;
use rustfft::FFT;

type Res<T> = Result<T, Box<dyn error::Error>>;

use crate::ResamplerError;

use crate::Resampler;




/// A resampler that accepts a fixed number of audio frames for input
/// and returns a variable number of frames.
///
/// The resampling is done by FFT:ing the input data. The spectrum is then extended or
/// truncated before it's reversed transformed to get the resampled waveforms
pub struct FFTFixedIn<T> {
    nbr_channels: usize,
    chunk_size: usize,
    fs_in: usize,
    fs_out: usize,
    fft_size_in: usize,
    fft_size_out: usize,
    filter: Vec<Complex<T>>,
    buffer: Vec<Vec<T>>,
    overlap: Vec<Vec<T>>,
}

/// A resampler that return a fixed number of audio frames.
/// The number of input frames required is given by the frames_needed function.
///
/// The resampling is done by creating a number of intermediate points (defined by oversampling_factor)
/// by sinc interpolation. The new samples are then calculated by interpolating between these points.
pub struct FFTFixedInOut<T> {
    nbr_channels: usize,
    chunk_size_in: usize,
    chunk_size_out: usize,
    fs_in: usize,
    fs_out: usize,
    fft_size_in: usize,
    fft_size_out: usize,
    filter_f: Vec<Complex<T>>,
    //buffer: Vec<Vec<T>>,
    overlaps: Vec<Vec<T>>,
    fft: std::sync::Arc<dyn rustfft::FFT<T>>,
    ifft: std::sync::Arc<dyn rustfft::FFT<T>>,
    input_buf: Vec<Complex<T>>,
    input_f: Vec<Complex<T>>,
    output_f: Vec<Complex<T>>,
    //temp_buf: Vec<Complex<T>>,
    output_buf: Vec<Complex<T>>,
}

macro_rules! impl_resampler {
    ($ft:ty, $rt:ty) => {
        impl $rt {
            /// Resample a small chunk
            fn resample_unit(&mut self, wave_in: &[$ft], wave_out: &mut [$ft], overlap_idx: usize) {
                // Copy to inut buffer and convert to complex
                for (n, item) in wave_in.iter().enumerate().take(self.fft_size_in) {
                    self.input_buf[n] = Complex::<$ft>::from(*item);
                    self.input_buf[n+self.fft_size_in] = Complex::zero();
                    //self.input_buf[n+self.npoints] = Complex::zero();
                }
            
                // FFT and store result in history, update index
                self.fft.process(&mut self.input_buf, &mut self.input_f);
            
                // multiply with filter FT
                for n in 0..2 * self.fft_size_in {
                    self.input_f[n] = self.input_f[n] * self.filter_f[n];
                }

                let new_len = if self.fft_size_in < self.fft_size_out {
                    self.fft_size_in
                }
                else {
                    self.fft_size_out
                };
                let offset_in = 2*self.fft_size_in - new_len;
                let offset_out = 2*self.fft_size_out - new_len;
                // copy to modified spectrum
                //println!("offset_in {}, offset_out {}, new_len{}", offset_in, offset_out, new_len);
                for n in 0..new_len {
                    self.output_f[n] = self.input_f[n];
                    self.output_f[n+offset_out] = self.input_f[n+offset_in];
                }
                for n in new_len..offset_out {
                    self.output_f[n] = Complex::zero();
                }

            
                // IFFT result, store result anv overlap
                self.ifft.process(&mut self.output_f, &mut self.output_buf);
                //let mut filtered: Vec<PrcFmt> = vec![0.0; self.npoints];
                for (n, item) in wave_out.iter_mut().enumerate().take(self.fft_size_out) {
                    *item = self.output_buf[n].re + self.overlaps[overlap_idx][n];
                    self.overlaps[overlap_idx][n] = self.output_buf[n + self.fft_size_out].re;
                }
            }

        }
    };
}
//impl_resampler!(f32, FFTFixedIn<f32>);
//impl_resampler!(f64, FFTFixedIn<f64>);
impl_resampler!(f32, FFTFixedInOut<f32>);
impl_resampler!(f64, FFTFixedInOut<f64>);

macro_rules! impl_fixedinout {
    ($ft:ty) => {
        impl FFTFixedInOut<$ft> {
            /// Create a new FFTFixedInOut
            ///
            /// Parameters are:
            /// - `resample_ratio`: Ratio between output and input sample rates.
            /// - `parameters`: Parameters for interpolation, see `InterpolationParameters`
            /// - `chunk_size`: size of input data in frames
            /// - `nbr_channels`: number of channels in input/output
            pub fn new(
                fs_in: usize,
                fs_out: usize,
                chunk_size_in: usize,
                nbr_channels: usize,
            ) -> Self {
                println!(
                    "Create new FFTFixedInOut, fs_in: {}, fs_out: {} chunk_size_in: {}, channels: {}",
                    fs_in, fs_out, chunk_size_in, nbr_channels
                );
                //let sinc_cutoff = if resample_ratio >= 1.0 {
                //    parameters.f_cutoff
                //} else {
                //    parameters.f_cutoff * resample_ratio as f32
                //};
                //let sinc_cutoff=0.95;
                
                
                let gcd = integer::gcd(fs_in, fs_out);
                let min_chunk_out = fs_out/gcd;
                let wanted = chunk_size_in;
                let fft_chunks = (wanted as f32 / min_chunk_out as f32).ceil() as usize;
                let fft_size_out = fft_chunks * fs_out / gcd;
                let fft_size_in = fft_chunks * fs_in / gcd;

                let sinc = make_sincs::<$ft>(fft_size_in, 1, 0.95, WindowFunction::BlackmanHarris2);
                let mut filter_t: Vec<Complex<$ft>> = vec![Complex::zero(); 2*fft_size_in];
                let mut filter_f: Vec<Complex<$ft>> = vec![Complex::zero(); 2*fft_size_in];
                let half_len = fft_size_in/2;
                //for n in 0..(fft_size_in-half_len) {
                //    filter_t[n] = Complex::from(sinc[0][n+half_len]/(2.0 * fft_size_in as $ft));
                //}
                //for n in 0..half_len {
                //    filter_t[n+2*fft_size_in-half_len-1] = Complex::from(sinc[0][n]/(2.0 * fft_size_in as $ft));
                //}
                for n in 0..fft_size_in {
                    filter_t[n] = Complex::from(sinc[0][n]/(2.0 * fft_size_in as $ft));
                }

                //let sinc_len = 8 * (((parameters.sinc_len as f32) / 8.0).ceil() as usize);
                let input_f: Vec<Complex<$ft>> = vec![Complex::zero(); 2 * fft_size_in];
                let input_buf: Vec<Complex<$ft>> = vec![Complex::zero(); 2 * fft_size_in];
                let overlaps: Vec<Vec<$ft>> = vec![vec![0.0; fft_size_out]; nbr_channels];
                let output_f: Vec<Complex<$ft>> = vec![Complex::zero(); 2 * fft_size_out];
                let output_buf: Vec<Complex<$ft>> = vec![Complex::zero(); 2 * fft_size_out];
                let mut fft_planner = FFTplanner::<$ft>::new(false);
                let mut ifft_planner = FFTplanner::<$ft>::new(true);
                let fft = fft_planner.plan_fft(2*fft_size_in);
                let ifft = ifft_planner.plan_fft(2*fft_size_out);
                
                let input_f = vec![Complex::zero(); 2 * fft_size_in];
                
                fft.process(&mut filter_t, &mut filter_f);

                println!("Resampler from {} to {} frames", fft_size_in, fft_size_out);
                
                //for (n, coeff) in coeffs.iter().enumerate() {
                //    coeffs_c[n / data_length][n % data_length] =
                //        Complex::from(coeff / (2.0 * data_length as PrcFmt));
                //}
                
                FFTFixedInOut {
                    nbr_channels,
                    chunk_size_in: fft_size_in,
                    chunk_size_out: fft_size_out,
                    fs_in,
                    fs_out,
                    fft_size_in,
                    fft_size_out,
                    filter_f,
                    //buffer: Vec<Vec<T>>,
                    overlaps,
                    fft,
                    ifft,
                    input_buf,
                    input_f,
                    output_f,
                    output_buf,
                }
            }
        }
    }
}
impl_fixedinout!(f64);

macro_rules! resampler_sincfixedinout {
    ($t:ty) => {
        impl Resampler<$t> for FFTFixedInOut<$t> {
            /// Query for the number of frames needed for the next call to "process".
            fn nbr_frames_needed(&self) -> usize {
                self.fft_size_in
            }

            /// Update the resample ratio. New value must be within +-10% of the original one
            fn set_resample_ratio(&mut self, new_ratio: f64) -> Res<()> {
                Err(Box::new(ResamplerError::new("Not possible to adjust a synchronous resampler)")))
            }

            /// Update the resample ratio relative to the original one
            fn set_resample_ratio_relative(&mut self, rel_ratio: f64) -> Res<()> {
                Err(Box::new(ResamplerError::new("Not possible to adjust a synchronous resampler)")))
            }

            /// Resample a chunk of audio. The input and output lengths are fixed.
            /// # Errors
            ///
            /// The function returns an error if the size of the input data is not equal
            /// to the number of channels and input size defined when creating the instance.
            fn process(&mut self, wave_in: &[Vec<$t>]) -> Res<Vec<Vec<$t>>> {
                if wave_in.len() != self.nbr_channels {
                    return Err(Box::new(ResamplerError::new(
                        "Wrong number of channels in input",
                    )));
                }
                if wave_in[0].len() != self.chunk_size_in {
                    return Err(Box::new(ResamplerError::new(
                        "Wrong number of frames in input",
                    )));
                }
                let mut wave_out=vec![vec![0.0 as $t;self.chunk_size_out]; self.nbr_channels];
                for n in 0..self.nbr_channels {
                    self.resample_unit(&wave_in[n], &mut wave_out[n], n)
                }
                Ok(wave_out)
            }
        }
    }
}
resampler_sincfixedinout!(f32);
resampler_sincfixedinout!(f64);

//macro_rules! resampler_sincfixedin {
//    ($t:ty) => {
//        impl Resampler<$t> for SincFixedIn<$t> {
//            /// Resample a chunk of audio. The input length is fixed, and the output varies in length.
//            /// # Errors
//            ///
//            /// The function returns an error if the length of the input data is not equal
//            /// to the number of channels and chunk size defined when creating the instance.
//            fn process(&mut self, wave_in: &[Vec<$t>]) -> Res<Vec<Vec<$t>>> {
//                if wave_in.len() != self.nbr_channels {
//                    return Err(Box::new(ResamplerError::new(
//                        "Wrong number of channels in input",
//                    )));
//                }
//                if wave_in[0].len() != self.chunk_size {
//                    return Err(Box::new(ResamplerError::new(
//                        "Wrong number of frames in input",
//                    )));
//                }
//                let end_idx = self.chunk_size as isize - (self.sinc_len as isize + 1);
//                //update buffer with new data
//                for wav in self.buffer.iter_mut() {
//                    for idx in 0..(2 * self.sinc_len) {
//                        wav[idx] = wav[idx + self.chunk_size];
//                    }
//                }
//                for (chan, wav) in wave_in.iter().enumerate() {
//                    for (idx, sample) in wav.iter().enumerate() {
//                        self.buffer[chan][idx + 2 * self.sinc_len] = *sample;
//                    }
//                }
//
//                let mut idx = self.last_index;
//                let t_ratio = 1.0 / self.resample_ratio as f64;
//
//                let mut wave_out = vec![
//                    vec![
//                        0.0 as $t;
//                        (self.chunk_size as f64 * self.resample_ratio + 10.0)
//                            as usize
//                    ];
//                    self.nbr_channels
//                ];
//                let mut n = 0;
//
//                match self.interpolation {
//                    InterpolationType::Cubic => {
//                        let mut points = vec![0.0 as $t; 4];
//                        let mut nearest = vec![(0isize, 0isize); 4];
//                        while idx < end_idx as f64 {
//                            idx += t_ratio;
//                            get_nearest_times_4(
//                                idx,
//                                self.oversampling_factor as isize,
//                                &mut nearest,
//                            );
//                            let frac = idx * self.oversampling_factor as f64
//                                - (idx * self.oversampling_factor as f64).floor();
//                            let frac_offset = frac as $t;
//                            for (chan, buf) in self.buffer.iter().enumerate() {
//                                for (n, p) in nearest.iter().zip(points.iter_mut()) {
//                                    *p = self.get_sinc_interpolated(
//                                        &buf,
//                                        (n.0 + 2 * self.sinc_len as isize) as usize,
//                                        n.1 as usize,
//                                    );
//                                }
//                                wave_out[chan][n] = self.interp_cubic(frac_offset, &points);
//                            }
//                            n += 1;
//                        }
//                    }
//                    InterpolationType::Linear => {
//                        let mut points = vec![0.0 as $t; 2];
//                        let mut nearest = vec![(0isize, 0isize); 2];
//                        while idx < end_idx as f64 {
//                            idx += t_ratio;
//                            get_nearest_times_2(
//                                idx,
//                                self.oversampling_factor as isize,
//                                &mut nearest,
//                            );
//                            let frac = idx * self.oversampling_factor as f64
//                                - (idx * self.oversampling_factor as f64).floor();
//                            let frac_offset = frac as $t;
//                            for (chan, buf) in self.buffer.iter().enumerate() {
//                                for (n, p) in nearest.iter().zip(points.iter_mut()) {
//                                    *p = self.get_sinc_interpolated(
//                                        &buf,
//                                        (n.0 + 2 * self.sinc_len as isize) as usize,
//                                        n.1 as usize,
//                                    );
//                                }
//                                wave_out[chan][n] = self.interp_lin(frac_offset, &points);
//                            }
//                            n += 1;
//                        }
//                    }
//                    InterpolationType::Nearest => {
//                        let mut point;
//                        let mut nearest;
//                        while idx < end_idx as f64 {
//                            idx += t_ratio;
//                            nearest = get_nearest_time(idx, self.oversampling_factor as isize);
//                            for (chan, buf) in self.buffer.iter().enumerate() {
//                                point = self.get_sinc_interpolated(
//                                    &buf,
//                                    (nearest.0 + 2 * self.sinc_len as isize) as usize,
//                                    nearest.1 as usize,
//                                );
//                                wave_out[chan][n] = point;
//                            }
//                            n += 1;
//                        }
//                    }
//                }
//
//                // store last index for next iteration
//                self.last_index = idx - self.chunk_size as f64;
//                for w in wave_out.iter_mut() {
//                    w.truncate(n);
//                }
//                trace!(
//                    "Resampling, {} frames in, {} frames out",
//                    wave_in[0].len(),
//                    wave_out[0].len()
//                );
//                Ok(wave_out)
//            }
//
//            /// Update the resample ratio. New value must be within +-10% of the original one
//            fn set_resample_ratio(&mut self, new_ratio: f64) -> Res<()> {
//                trace!("Change resample ratio to {}", new_ratio);
//                if (new_ratio / self.resample_ratio_original > 0.9)
//                    && (new_ratio / self.resample_ratio_original < 1.1)
//                {
//                    self.resample_ratio = new_ratio;
//                    Ok(())
//                } else {
//                    Err(Box::new(ResamplerError::new(
//                        "New resample ratio is too far off from original",
//                    )))
//                }
//            }
//            /// Update the resample ratio relative to the original one
//            fn set_resample_ratio_relative(&mut self, rel_ratio: f64) -> Res<()> {
//                let new_ratio = self.resample_ratio_original * rel_ratio;
//                self.set_resample_ratio(new_ratio)
//            }
//
//            /// Query for the number of frames needed for the next call to "process".
//            /// Will always return the chunk_size defined when creating the instance.
//            fn nbr_frames_needed(&self) -> usize {
//                self.chunk_size
//            }
//        }
//    };
//}
//resampler_sincfixedin!(f32);
//resampler_sincfixedin!(f64);

//impl<T: Float> SincFixedOut<T> {
//    /// Create a new SincFixedOut
//    ///
//    /// Parameters are:
//    /// - `resample_ratio`: Ratio between output and input sample rates.
//    /// - `parameters`: Parameters for interpolation, see `InterpolationParameters`
//    /// - `chunk_size`: size of output data in frames
//    /// - `nbr_channels`: number of channels in input/output
//    pub fn new(
//        resample_ratio: f64,
//        parameters: InterpolationParameters,
//        chunk_size: usize,
//        nbr_channels: usize,
//    ) -> Self {
//        debug!(
//            "Create new SincFixedOut, ratio: {}, chunk_size: {}, channels: {}, parameters: {:?}",
//            resample_ratio, chunk_size, nbr_channels, parameters
//        );
//        let sinc_cutoff = if resample_ratio >= 1.0 {
//            parameters.f_cutoff
//        } else {
//            parameters.f_cutoff * resample_ratio as f32
//        };
//        let sinc_len = 8 * (((parameters.sinc_len as f32) / 8.0).ceil() as usize);
//        debug!("sinc_len rounded up to {}", sinc_len);
//        let sincs = make_sincs(
//            sinc_len,
//            parameters.oversampling_factor,
//            sinc_cutoff,
//            parameters.window,
//        );
//        let needed_input_size =
//            (chunk_size as f64 / resample_ratio).ceil() as usize + 2 + sinc_len / 2;
//        let buffer = vec![vec![T::zero(); 3 * needed_input_size / 2 + 2 * sinc_len]; nbr_channels];
//        SincFixedOut {
//            nbr_channels,
//            chunk_size,
//            needed_input_size,
//            oversampling_factor: parameters.oversampling_factor,
//            last_index: -((sinc_len / 2) as f64),
//            current_buffer_fill: needed_input_size,
//            resample_ratio,
//            resample_ratio_original: resample_ratio,
//            sinc_len,
//            sincs,
//            buffer,
//            interpolation: parameters.interpolation,
//        }
//    }
//}
//
//macro_rules! resampler_sincfixedout {
//    ($t:ty) => {
//        impl Resampler<$t> for SincFixedOut<$t> {
//            /// Query for the number of frames needed for the next call to "process".
//            fn nbr_frames_needed(&self) -> usize {
//                self.needed_input_size
//            }
//
//            /// Update the resample ratio. New value must be within +-10% of the original one
//            fn set_resample_ratio(&mut self, new_ratio: f64) -> Res<()> {
//                trace!("Change resample ratio to {}", new_ratio);
//                if (new_ratio / self.resample_ratio_original > 0.9)
//                    && (new_ratio / self.resample_ratio_original < 1.1)
//                {
//                    self.resample_ratio = new_ratio;
//                    self.needed_input_size = (self.last_index as f32
//                        + self.chunk_size as f32 / self.resample_ratio as f32
//                        + self.sinc_len as f32)
//                        .ceil() as usize
//                        + 2;
//                    Ok(())
//                } else {
//                    Err(Box::new(ResamplerError::new(
//                        "New resample ratio is too far off from original",
//                    )))
//                }
//            }
//
//            /// Update the resample ratio relative to the original one
//            fn set_resample_ratio_relative(&mut self, rel_ratio: f64) -> Res<()> {
//                let new_ratio = self.resample_ratio_original * rel_ratio;
//                self.set_resample_ratio(new_ratio)
//            }
//
//            /// Resample a chunk of audio. The required input length is provided by
//            /// the "nbr_frames_required" function, and the output length is fixed.
//            /// # Errors
//            ///
//            /// The function returns an error if the length of the input data is not
//            /// equal to the number of channels defined when creating the instance,
//            /// and the number of audio frames given by "nbr_frames"required".
//            fn process(&mut self, wave_in: &[Vec<$t>]) -> Res<Vec<Vec<$t>>> {
//                //update buffer with new data
//                if wave_in.len() != self.nbr_channels {
//                    return Err(Box::new(ResamplerError::new(
//                        "Wrong number of channels in input",
//                    )));
//                }
//                if wave_in[0].len() != self.needed_input_size {
//                    return Err(Box::new(ResamplerError::new(
//                        "Wrong number of frames in input",
//                    )));
//                }
//                for wav in self.buffer.iter_mut() {
//                    for idx in 0..(2 * self.sinc_len) {
//                        wav[idx] = wav[idx + self.current_buffer_fill];
//                    }
//                }
//                self.current_buffer_fill = wave_in[0].len();
//                for (chan, wav) in wave_in.iter().enumerate() {
//                    for (idx, sample) in wav.iter().enumerate() {
//                        self.buffer[chan][idx + 2 * self.sinc_len] = *sample;
//                    }
//                }
//
//                let mut idx = self.last_index;
//                let t_ratio = 1.0 / self.resample_ratio as f64;
//
//                let mut wave_out = vec![vec![0.0 as $t; self.chunk_size]; self.nbr_channels];
//
//                match self.interpolation {
//                    InterpolationType::Cubic => {
//                        let mut points = vec![0.0 as $t; 4];
//                        let mut nearest = vec![(0isize, 0isize); 4];
//                        for n in 0..self.chunk_size {
//                            idx += t_ratio;
//                            get_nearest_times_4(idx, self.oversampling_factor as isize, &mut nearest);
//                            let frac = idx * self.oversampling_factor as f64
//                                - (idx * self.oversampling_factor as f64).floor();
//                            let frac_offset = frac as $t;
//                            for (chan, buf) in self.buffer.iter().enumerate() {
//                                for (n, p) in nearest.iter().zip(points.iter_mut()) {
//                                    *p = self.get_sinc_interpolated(
//                                        &buf,
//                                        (n.0 + 2 * self.sinc_len as isize) as usize,
//                                        n.1 as usize,
//                                    );
//                                }
//                                wave_out[chan][n] = self.interp_cubic(frac_offset, &points);
//                            }
//                        }
//                    }
//                    InterpolationType::Linear => {
//                        let mut points = vec![0.0 as $t; 2];
//                        let mut nearest = vec![(0isize, 0isize); 2];
//                        for n in 0..self.chunk_size {
//                            idx += t_ratio;
//                            get_nearest_times_2(idx, self.oversampling_factor as isize, &mut nearest);
//                            let frac = idx * self.oversampling_factor as f64
//                                - (idx * self.oversampling_factor as f64).floor();
//                            let frac_offset = frac as $t;
//                            for (chan, buf) in self.buffer.iter().enumerate() {
//                                for (n, p) in nearest.iter().zip(points.iter_mut()) {
//                                    *p = self.get_sinc_interpolated(
//                                        &buf,
//                                        (n.0 + 2 * self.sinc_len as isize) as usize,
//                                        n.1 as usize,
//                                    );
//                                }
//                                wave_out[chan][n] = self.interp_lin(frac_offset, &points);
//                            }
//                        }
//                    }
//                    InterpolationType::Nearest => {
//                        let mut point;
//                        let mut nearest;
//                        for n in 0..self.chunk_size {
//                            idx += t_ratio;
//                            nearest = get_nearest_time(idx, self.oversampling_factor as isize);
//                            for (chan, buf) in self.buffer.iter().enumerate() {
//                                point = self.get_sinc_interpolated(
//                                    &buf,
//                                    (nearest.0 + 2 * self.sinc_len as isize) as usize,
//                                    nearest.1 as usize,
//                                );
//                                wave_out[chan][n] = point;
//                            }
//                        }
//                    }
//                }
//
//                // store last index for next iteration
//                //trace!("idx {}, fill{}", idx, self.current_buffer_fill);
//                self.last_index = idx - self.current_buffer_fill as f64;
//                //let next_last_index = self.last_index as f64 + self.chunk_size as f64 / self.resample_ratio as f64 + self.sinc_len as f64;
//                //let needed_with_margin = next_last_index + (self.sinc_len) as f64;
//                self.needed_input_size = (self.last_index as f32
//                    + self.chunk_size as f32 / self.resample_ratio as f32
//                    + self.sinc_len as f32)
//                    .ceil() as usize
//                    + 2;
//                //self.needed_input_size = ((self.chunk_size as f32 + self.last_index as f32 + (self.sinc_len) as f32)/ self.resample_ratio).ceil() as usize + 2;
//                //self.needed_input_size = (self.needed_input_size as isize
//                //    + self.last_index.round() as isize
//                //    + self.sinc_len as isize) as usize + 2;
//                trace!(
//                    "Resampling, {} frames in, {} frames out. Next needed length: {} frames, last index {}",
//                    wave_in[0].len(),
//                    wave_out[0].len(),
//                    self.needed_input_size,
//                    self.last_index
//                );
//                Ok(wave_out)
//            }
//        }
//    }
//}
//resampler_sincfixedout!(f32);
//resampler_sincfixedout!(f64);
//
#[cfg(test)]
mod tests {
    use crate::Resampler;
    use crate::synchro::FFTFixedInOut;

    

    #[test]
    fn make_resampler_fo() {
        let mut resampler = FFTFixedInOut::<f64>::new(44100, 48000, 1024, 1);
        let mut wave_in = vec![0.0; resampler.fft_size_in];

        wave_in[0] = 1.0;
        let mut wave_out = vec![0.0; resampler.fft_size_out];
        let mut overlap = vec![0.0; resampler.fft_size_out];
        resampler.resample_unit(&wave_in, &mut wave_out, &mut overlap);
        assert_eq!(wave_out[0],1.0);
    }


}