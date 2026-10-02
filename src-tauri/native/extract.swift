import Foundation
import PDFKit
import ImageIO
import CoreGraphics

struct Page: Codable {let page: Int;let text: String?;let mime_type: String?;let image_base64: String?;let width: Int?;let height: Int?}
struct Extraction: Codable {let schema_version: Int;let kind: String;let mime_type: String;let width: Int?;let height: Int?;let image_base64: String?;let pages: [Page];let warnings: [String];let total_pages: Int?}
func fail(_ code: String) -> Never {FileHandle.standardError.write(Data(code.utf8));exit(2)}
let input = FileHandle.standardInput.readDataToEndOfFile()
if input.count > 100 * 1024 * 1024 {fail("file_byte_limit")}
let args = CommandLine.arguments
if args.count < 2 || args.count > 4 {fail("invalid_worker_request")}
var output: Extraction
if args[1] == "application/pdf" {
 guard let doc = PDFDocument(data: input), !doc.isEncrypted else {fail("pdf_invalid_or_encrypted")}
 if doc.pageCount < 1 || doc.pageCount > 1000 {fail("pdf_hard_page_limit")}
 if args.count == 3 {
  if args[2] != "--metadata" {fail("invalid_worker_request")}
  let metadata=Extraction(schema_version:1,kind:"pdf_text",mime_type:"application/pdf",width:nil,height:nil,image_base64:nil,pages:[],warnings:[],total_pages:doc.pageCount)
  do {FileHandle.standardOutput.write(try JSONEncoder().encode(metadata));exit(0)} catch {fail("worker_output_failed")}
 }
 var first = 1;var last = doc.pageCount
 if args.count == 4 {
  guard let start=Int(args[2]),let end=Int(args[3]),start>=1,end>=start,end<=doc.pageCount else {fail("pdf_invalid_page_range")}
  first=start;last=end
 }
 if last-first+1 > 200 {fail("pdf_page_limit_select_range")}
 var pages = [Page]()
 var hasRaster = false
 for i in (first-1)..<last {
  guard let page = doc.page(at: i) else {fail("pdf_page_unavailable")}
  let text = page.string ?? ""
  if text.utf8.count > 1024 * 1024 {fail("page_text_limit")}
  if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
   pages.append(Page(page: i+1,text: text,mime_type: nil,image_base64: nil,width: nil,height: nil))
  } else {
   hasRaster = true
   let bounds = page.bounds(for: .mediaBox)
   let width = Int(ceil(bounds.width * 1.5));let height = Int(ceil(bounds.height * 1.5))
   if width <= 0 || height <= 0 || width > 10000 || height > 10000 || width * height > 40_000_000 {fail("pdf_raster_pixel_limit")}
   guard let context = CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:width*4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else {fail("pdf_raster_allocation")}
   context.setFillColor(CGColor(gray:1,alpha:1));context.fill(CGRect(x:0,y:0,width:width,height:height));context.scaleBy(x:1.5,y:1.5)
   page.draw(with: .mediaBox,to:context)
   guard let image=context.makeImage() else {fail("pdf_raster_failed")}
   let data=NSMutableData()
   guard let destination=CGImageDestinationCreateWithData(data,"public.png" as CFString,1,nil) else {fail("pdf_raster_encoder")}
   CGImageDestinationAddImage(destination,image,nil)
   if !CGImageDestinationFinalize(destination) {fail("pdf_raster_encoder")}
   pages.append(Page(page:i+1,text:nil,mime_type:"image/png",image_base64:(data as Data).base64EncodedString(),width:width,height:height))
  }
 }
 output=Extraction(schema_version:1,kind:hasRaster ? "pdf_raster":"pdf_text",mime_type:"application/pdf",width:nil,height:nil,image_base64:nil,pages:pages,warnings:["Reading order and table structure may be lost; page mapping preserved.","No OCR substitution; scanned pages require a vision-capable model."],total_pages:doc.pageCount)
} else if args[1] == "image/png" || args[1] == "image/jpeg" {
 if args.count != 2 {fail("invalid_worker_request")}
 guard let source=CGImageSourceCreateWithData(input as CFData,[kCGImageSourceShouldCache:false] as CFDictionary),CGImageSourceGetCount(source)==1,
 let imageType=CGImageSourceGetType(source), (imageType as String)==(args[1]=="image/png" ? "public.png":"public.jpeg"),
 let properties=CGImageSourceCopyPropertiesAtIndex(source,0,nil) as? [CFString:Any],let width=properties[kCGImagePropertyPixelWidth] as? Int,let height=properties[kCGImagePropertyPixelHeight] as? Int else {fail("image_invalid_or_multiframe")}
 if width<=0 || height<=0 || width>10000 || height>10000 || width*height>40_000_000 {fail("image_pixel_limit")}
 guard CGImageSourceCreateImageAtIndex(source,0,[kCGImageSourceShouldCache:false] as CFDictionary) != nil else {fail("image_decode_failed")}
 output=Extraction(schema_version:1,kind:"image",mime_type:args[1],width:width,height:height,image_base64:input.base64EncodedString(),pages:[],warnings:["Image bytes require a vision-capable model; no OCR substitution."],total_pages:nil)
} else {fail("unsupported_worker_mime")}
do {let bytes=try JSONEncoder().encode(output);if bytes.count>100*1024*1024{fail("derived_byte_limit")};FileHandle.standardOutput.write(bytes)} catch {fail("worker_output_failed")}
