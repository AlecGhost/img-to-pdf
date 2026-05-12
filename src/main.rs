use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lopdf::Document;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use img_to_pdf::{ImageData, create_pdf, insert_pages, remove_page, swap_pages};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Output PDF file
    #[arg(short, long)]
    output: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to an image or a folder of images to convert to PDF.
    /// Supports multiple files via shell globbing.
    paths: Option<Vec<PathBuf>>,
}

#[derive(Subcommand)]
enum Commands {
    /// Insert images at a page number into an existing PDF
    Insert {
        /// The target PDF file
        pdf: PathBuf,
        /// The images to insert, followed by the page number.
        /// Example: insert file.pdf img1.png img2.png 1
        #[arg(required = true)]
        images_and_number: Vec<String>,
    },
    /// Remove a page from an existing PDF
    Remove {
        /// The target PDF file
        pdf: PathBuf,
        /// The page number to remove (1-based index)
        number: u32,
    },
    /// Swap two pages in an existing PDF
    Swap {
        /// The target PDF file
        pdf: PathBuf,
        /// The first page number (1-based index)
        number1: u32,
        /// The second page number (1-based index)
        number2: u32,
    },
}

fn expand_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut expanded = Vec::new();
    for p in paths {
        if p.is_dir() {
            for entry in WalkDir::new(p).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file() {
                    expanded.push(path.to_path_buf());
                }
            }
        } else {
            expanded.push(p.clone());
        }
    }
    // Sort paths alphabetically
    expanded.sort();
    Ok(expanded)
}

fn read_image_data(path: &Path) -> Result<ImageData> {
    let data = std::fs::read(path).with_context(|| format!("Could not find or read the file named '{}'. If you meant to use a page number, ensure you provided the correct subcommand (like 'insert', 'remove', or 'swap').", path.display()))?;

    let reader = exif::Reader::new();
    let mut orientation = 1;
    if let Ok(exif) = reader.read_from_container(&mut std::io::Cursor::new(&data))
        && let Some(field) = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
            && let exif::Value::Short(ref v) = field.value
                && !v.is_empty() {
                    orientation = v[0];
                }

    // Check if it's a JPEG
    let is_jpeg = data.starts_with(&[0xFF, 0xD8]);

    if is_jpeg {
        let dimensions = image::image_dimensions(path).with_context(|| format!("Could not determine the dimensions of the JPEG image at '{}'. The file might be corrupted or in an unsupported format.", path.display()))?;
        Ok(ImageData {
            data,
            width: dimensions.0,
            height: dimensions.1,
            orientation,
        })
    } else {
        // Not a JPEG (e.g. PNG, WebP). lopdf has bugs with PNG alpha channels (rendering as white).
        // It also doesn't support WebP/HEIC natively.
        // We decode it using the `image` crate, drop the alpha channel by converting to RGB8,
        // and re-encode to a standard JPEG in memory. We DO NOT rotate the pixels here,
        // we preserve the EXIF orientation to be applied via PDF /Rotate.
        let img = image::load_from_memory(&data).with_context(|| format!("Could not decode the image file at '{}'. Please ensure it is a valid, supported image format (like PNG, WebP, or HEIC).", path.display()))?;
        let rgb_img = img.into_rgb8();

        let mut new_data = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut new_data);
        rgb_img.write_to(&mut cursor, image::ImageFormat::Jpeg).context("An unexpected error occurred while re-encoding the image to JPEG for PDF compatibility. The image data might be invalid.")?;

        Ok(ImageData {
            data: new_data,
            width: rgb_img.width(),
            height: rgb_img.height(),
            orientation,
        })
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        match cmd {
            Commands::Insert {
                pdf,
                images_and_number,
            } => {
                if images_and_number.len() < 2 {
                    anyhow::bail!(
                        "The 'insert' command requires at least two arguments: one or more images, followed by the page number where they should be inserted. Example: img-to-pdf insert my_pdf.pdf image1.png 3"
                    );
                }

                let mut images_and_number = images_and_number;
                let number_str = images_and_number.pop().unwrap();
                let number: u32 = number_str.parse().context("The final argument for the 'insert' command must be a valid, positive page number.")?;

                let image_paths: Vec<PathBuf> =
                    images_and_number.into_iter().map(PathBuf::from).collect();
                let expanded_paths = expand_paths(&image_paths)?;

                if expanded_paths.len() > 1 && cli.output.is_none() {
                    // For inserting into an existing PDF, the target PDF is known and mutated in-place by default.
                }

                let mut images_data = Vec::new();
                for p in expanded_paths {
                    let img = read_image_data(&p)?;
                    images_data.push(img);
                }

                let mut doc = Document::load(&pdf).context("Could not open the target PDF file. Please verify that the file exists, is a valid PDF document, and is not locked by another program.")?;
                insert_pages(&mut doc, &images_data, number)?;

                let output_path = cli.output.as_ref().unwrap_or(&pdf);
                doc.save(output_path).context("Could not save the resulting PDF. Please check if you have write permissions for the destination path.")?;
            }
            Commands::Remove { pdf, number } => {
                let mut doc = Document::load(&pdf).context("Could not open the target PDF file. Please verify that the file exists, is a valid PDF document, and is not locked by another program.")?;
                remove_page(&mut doc, number)?;

                let output_path = cli.output.as_ref().unwrap_or(&pdf);
                doc.save(output_path).context("Could not save the resulting PDF. Please check if you have write permissions for the destination path.")?;
            }
            Commands::Swap {
                pdf,
                number1,
                number2,
            } => {
                let mut doc = Document::load(&pdf).context("Could not open the target PDF file. Please verify that the file exists, is a valid PDF document, and is not locked by another program.")?;
                swap_pages(&mut doc, number1, number2)?;

                let output_path = cli.output.as_ref().unwrap_or(&pdf);
                doc.save(output_path).context("Could not save the resulting PDF. Please check if you have write permissions for the destination path.")?;
            }
        }
    } else {
        // Create mode
        let paths = cli.paths.unwrap_or_default();
        if paths.is_empty() {
            anyhow::bail!(
                "You must provide either a command (like 'insert', 'remove', 'swap') or a list of images/folders to create a new PDF."
            );
        }
        let expanded_paths = expand_paths(&paths)?;

        if expanded_paths.is_empty() {
            anyhow::bail!(
                "No valid images were found in the provided paths or folders. Please provide at least one valid image file."
            );
        }

        let output_path = if let Some(out) = cli.output {
            out
        } else {
            if paths.len() > 1 || paths[0].is_dir() {
                anyhow::bail!(
                    "When creating a PDF from multiple files or a folder, you must explicitly specify the output file name using the '-o' or '--output' flag. Example: img-to-pdf my_folder -o result.pdf"
                );
            }
            // default to file_stem.pdf
            let mut out = paths[0].clone();
            out.set_extension("pdf");

            if out.exists() {
                anyhow::bail!(
                    "The default output file '{}' already exists. To prevent accidental data loss, the file will not be overwritten. Please use the '-o' flag to explicitly specify the output file, or delete the existing file.",
                    out.display()
                );
            }
            out
        };

        let mut images_data = Vec::new();
        for p in expanded_paths {
            let img = read_image_data(&p)?;
            images_data.push(img);
        }

        let mut doc = create_pdf(&images_data)?;
        doc.save(&output_path).context("Could not save the resulting PDF. Please check if you have write permissions for the destination path.")?;
    }

    Ok(())
}
