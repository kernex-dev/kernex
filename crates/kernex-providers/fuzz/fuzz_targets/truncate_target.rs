#![no_main]

use libfuzzer_sys::fuzz_target;
use kernex_providers::tools::truncate_output;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    
    // Take the first 2 bytes as a truncation length pseudo-randomizer
    let max_bytes = u16::from_le_bytes([data[0], data[1]]) as usize;
    let text_data = &data[2..];
    
    // If the fuzzer generates valid UTF-8, test the truncation logic
    if let Ok(s) = std::str::from_utf8(text_data) {
        let truncated = truncate_output(s, max_bytes);
        
        if s.len() <= max_bytes {
            assert_eq!(truncated, s);
        } else {
            let limit = s.floor_char_boundary(max_bytes);
            assert!(truncated.starts_with(&s[..limit]));
            assert!(truncated.contains("... (output truncated:"));
        }
    }
});
