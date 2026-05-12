# img-to-pdf

> "We don't accept images, please provide this as a PDF"
>
> -- every bureaucrat ever

When run with just a file/folder/glob as argument, this puts your image(s) in a new PDF.
The subcommands `insert`, `remove` and `swap` help you edit an existing PDF.

~~~bash
Usage: img-to-pdf [OPTIONS] [PATHS]... [COMMAND]

Commands:
  insert  Insert images at a page number into an existing PDF
  remove  Remove a page from an existing PDF
  swap    Swap two pages in an existing PDF
  help    Print this message or the help of the given subcommand(s)

Arguments:
  [PATHS]...  Path to an image or a folder of images to convert to PDF. Supports multiple files via shell globbing

Options:
  -o, --output <OUTPUT>  Output PDF file
  -h, --help             Print help
  -V, --version          Print version
~~~
