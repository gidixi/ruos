# Test init: launch the egui desktop for a few frames (headless), then print a
# marker. Proves gui.cwasm (gui-core + ruos-backend) runs end-to-end in ruos.
echo ruos boot OK
gui --frames=3
echo GUI-DONE
