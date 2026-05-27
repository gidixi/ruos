NOTE (2026-05): The project pivoted to a Rust no_std kernel booted by Limine
(see ../docs/superpowers/roadmap-rust-os.md and the top-level Makefile). This
x64barebones/ C tree is kept as REFERENCE ONLY and is no longer built or run.
The cross-gcc Toolchain/ was removed, so the steps below (and Image/Makefile's
ModulePacker dependency) no longer work. Do not follow them for the active OS.

----------------------------------------------------------------------

x64BareBones is a basic setup to develop operating systems for the Intel 64 bits architecture.

The final goal of the project is to provide an entry point for a kernel and the possibility to load extra binary modules separated from the main kernel.

Environment setup:
1- Install the following packages before building the Toolchain and Kernel:

nasm qemu gcc make

2- Build the Toolchain

Execute the following commands on the x64BareBones project directory:

  user@linux:$ cd Toolchain
  user@linux:$ make all

3- Build the Kernel

From the x64BareBones project directory run:

  user@linux:$ make all

4- Run the kernel

From the x64BareBones project directory run:

  user@linux:$ ./run.sh


Author: Rodrigo Rearden (RowDaBoat)
Collaborator: Augusto Nizzo McIntosh