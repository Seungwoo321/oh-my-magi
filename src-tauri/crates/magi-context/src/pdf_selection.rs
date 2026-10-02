use crate::NativeExtraction;

pub fn captured_pdf_page_count(captured_pdf: &[u8]) -> Result<u32, String> {
    crate::extraction::extract_native_request(captured_pdf, "application/pdf", None, true)?
        .total_pages
        .ok_or_else(|| "PDF page count is unavailable.".into())
}

pub fn extract_pdf_page_selection(
    captured_pdf: &[u8],
    start_page: u32,
    end_page: u32,
) -> Result<NativeExtraction, String> {
    crate::extraction::extract_native_request(
        captured_pdf,
        "application/pdf",
        Some((start_page, end_page)),
        false,
    )
}
