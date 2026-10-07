use daw_core::audio_pool::{AudioFileRef, AudioMetadata};
use anyhow::Result;
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use time::{OffsetDateTime, macros::format_description};
use uuid::Uuid;

const BWF_CHUNK_ID: &[u8; 4] = b"bext";
const BWF_VERSION: u16 = 1;

pub struct BwfWriter {
    writer: WavWriter<BufWriter<File>>,
    path: std::path::PathBuf,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    metadata: AudioMetadata,
    samples_written: u64,
    data_chunk_pos: u64,
}

impl BwfWriter {
    pub fn create(
        path: impl AsRef<Path>,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
        metadata: AudioMetadata,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format: if bits_per_sample == 32 {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            },
        };
        
        let file = File::create(&path)?;
        let mut writer = WavWriter::new(BufWriter::new(file), spec)?;
        
        // Write placeholder for bext chunk - we'll write it at the end
        // Write placeholder data chunk size
        let data_chunk_pos = writer.len() as u64;
        
        let bwf = Self {
            writer,
            path,
            sample_rate,
            channels,
            bits_per_sample,
            metadata,
            samples_written: 0,
            data_chunk_pos,
        };
        
        Ok(bwf)
    }
    
    pub fn write_samples(&mut self, samples: &[f32]) -> Result<()> {
        for sample in samples {
            self.writer.write_sample(*sample)?;
            self.samples_written += 1;
        }
        Ok(())
    }
    
    pub fn write_samples_int(&mut self, samples: &[i32]) -> Result<()> {
        for sample in samples {
            self.writer.write_sample(*sample)?;
            self.samples_written += 1;
        }
        Ok(())
    }
    
    pub fn write_samples_i16(&mut self, samples: &[i16]) -> Result<()> {
        for sample in samples {
            self.writer.write_sample(*sample)?;
            self.samples_written += 1;
        }
        Ok(())
    }
    
    pub fn finalize(self) -> Result<()> {
        // Store path before finalize consumes self
        let path = self.path.clone();
        let metadata = self.metadata.clone();
        let sample_rate = self.sample_rate;
        let channels = self.channels;
        let bits_per_sample = self.bits_per_sample;
        
        // Finalize the WAV writer (writes data chunk size)
        self.writer.finalize()?;
        
        // Now we need to add the bext chunk
        // We need to rewrite the file to insert the bext chunk after the fmt chunk
        Self::insert_bext_chunk_static(
            path,
            sample_rate,
            channels,
            bits_per_sample,
            metadata,
        )?;
        
        Ok(())
    }
    
    fn insert_bext_chunk_static(
        path: std::path::PathBuf,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
        metadata: AudioMetadata,
    ) -> Result<()> {
        use std::io::{Read, Write};
        
        // Read the entire file
        let mut file = File::open(&path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        
        // Find the RIFF header and fmt chunk
        // RIFF header: 12 bytes (RIFF + size + WAVE)
        // fmt chunk: 24 bytes (fmt + size + data)
        // We need to insert bext chunk after fmt chunk
        
        // Find "fmt " chunk
        let fmt_pos = data.windows(4).position(|w| w == b"fmt ").ok_or_else(|| anyhow::anyhow!("fmt chunk not found"))?;
        
        // fmt chunk size is at fmt_pos + 4 (4 bytes)
        let fmt_chunk_size = u32::from_le_bytes([
            data[fmt_pos + 4],
            data[fmt_pos + 5],
            data[fmt_pos + 6],
            data[fmt_pos + 7],
        ]) as usize;
        
        // fmt chunk ends at fmt_pos + 8 + fmt_chunk_size
        let fmt_end = fmt_pos + 8 + fmt_chunk_size;
        
        // Build bext chunk
        let bext_data = Self::build_bext_chunk_static(sample_rate, channels, bits_per_sample, metadata);
        let bext_size = bext_data.len() as u32;
        
        // Insert bext chunk after fmt chunk
        let mut new_data = Vec::new();
        new_data.extend_from_slice(&data[..fmt_end]);
        new_data.extend_from_slice(b"bext");
        new_data.extend_from_slice(&bext_size.to_le_bytes());
        new_data.extend_from_slice(&bext_data);
        new_data.extend_from_slice(&data[fmt_end..]);
        
        // Update RIFF size (at offset 4, 4 bytes)
        let riff_size = (new_data.len() - 8) as u32;
        new_data[4..8].copy_from_slice(&riff_size.to_le_bytes());
        
        // Write back
        let mut file = File::create(&path)?;
        file.write_all(&new_data)?;
        file.flush()?;
        
        Ok(())
    }
    
    fn build_bext_chunk_static(
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
        metadata: AudioMetadata,
    ) -> Vec<u8> {
        let mut chunk = Vec::new();
        
        // Description (256 bytes)
        let mut description = [0u8; 256];
        let desc_bytes = metadata.comment.as_bytes();
        let len = desc_bytes.len().min(256);
        description[..len].copy_from_slice(&desc_bytes[..len]);
        chunk.extend_from_slice(&description);
        
        // Originator (32 bytes)
        let mut originator = [0u8; 32];
        let orig_bytes = metadata.originator.as_bytes();
        let len = orig_bytes.len().min(32);
        originator[..len].copy_from_slice(&orig_bytes[..len]);
        chunk.extend_from_slice(&originator);
        
        // OriginatorReference (32 bytes)
        let mut originator_ref = [0u8; 32];
        let ref_bytes = metadata.originator_ref.as_bytes();
        let len = ref_bytes.len().min(32);
        originator_ref[..len].copy_from_slice(&ref_bytes[..len]);
        chunk.extend_from_slice(&originator_ref);
        
        // OriginationDate (10 bytes) - YYYY-MM-DD
        let date_str = OffsetDateTime::now_utc()
            .format(&format_description!("[year]-[month]-[day]"))
            .unwrap_or_default();
        let mut orig_date = [0u8; 10];
        let date_bytes = date_str.as_bytes();
        orig_date[..date_bytes.len().min(10)].copy_from_slice(date_bytes);
        chunk.extend_from_slice(&orig_date);
        
        // OriginationTime (8 bytes) - HH:MM:SS
        let time_str = OffsetDateTime::now_utc()
            .format(&format_description!("[hour]:[minute]:[second]"))
            .unwrap_or_default();
        let mut orig_time = [0u8; 8];
        let time_bytes = time_str.as_bytes();
        orig_time[..time_bytes.len().min(8)].copy_from_slice(time_bytes);
        chunk.extend_from_slice(&orig_time);
        
        // TimeReferenceLow (4 bytes) - low 32 bits of sample count since midnight
        let time_ref = OffsetDateTime::now_utc().unix_timestamp() as u64 * sample_rate as u64;
        chunk.extend_from_slice(&(time_ref as u32).to_le_bytes());
        chunk.extend_from_slice(&(time_ref >> 32).to_le_bytes()); // TimeReferenceHigh
        
        // Version (2 bytes)
        chunk.extend_from_slice(&BWF_VERSION.to_le_bytes());
        
        // UMID (64 bytes)
        let mut umid = [0u8; 64];
        let umid_len = metadata.umid.len().min(64);
        umid[..umid_len].copy_from_slice(&metadata.umid[..umid_len]);
        chunk.extend_from_slice(&umid);
        
        // LoudnessValue (2 bytes) - not used
        chunk.extend_from_slice(&0u16.to_le_bytes());
        
        // LoudnessRange (2 bytes)
        chunk.extend_from_slice(&0u16.to_le_bytes());
        
        // MaxTruePeakLevel (2 bytes)
        chunk.extend_from_slice(&0u16.to_le_bytes());
        
        // MaxMomentaryLoudness (2 bytes)
        chunk.extend_from_slice(&0u16.to_le_bytes());
        
        // MaxShortTermLoudness (2 bytes)
        chunk.extend_from_slice(&0u16.to_le_bytes());
        
        // Reserved (180 bytes)
        chunk.extend_from_slice(&[0u8; 180]);
        
        // CodingHistory (variable, but we'll write what we have)
        let coding_history = metadata.coding_history.clone();
        let ch_bytes = coding_history.as_bytes();
        let ch_len = ch_bytes.len().min(256); // Limit to 256 bytes
        let mut ch_data = vec![0u8; 256];
        ch_data[..ch_len].copy_from_slice(&ch_bytes[..ch_len]);
        chunk.extend_from_slice(&ch_data);
        
        chunk
    }
    
    // Instance method for building bext chunk (kept for compatibility)
    fn build_bext_chunk(&self) -> Vec<u8> {
        Self::build_bext_chunk_static(self.sample_rate, self.channels, self.bits_per_sample, self.metadata.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    
    #[test]
    fn test_bwf_write() {
        let mut metadata = AudioMetadata::default();
        metadata.originator = "Zoft DAW".to_string();
        metadata.comment = "Test recording".to_string();
        
        let path = PathBuf::from("/tmp/test_bwf.wav");
        let mut writer = BwfWriter::create(&path, 48000, 2, 32, metadata).unwrap();
        
        // Write 1 second of silence
        let samples = vec![0.0f32; 48000 * 2];
        writer.write_samples(&samples).unwrap();
        
        writer.finalize().unwrap();
        
        // Cleanup
        std::fs::remove_file(path).ok();
    }
}