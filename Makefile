.SILENT:

.PHONY: dist clean

NAME = ukitouch-capture
OUT = ./dist
TARGET = ./target/release/$(NAME)
MANIFEST = ./manifest.json

build:
	cargo build --release
	mkdir -p $(OUT)
	cp $(TARGET) $(OUT)/
	cp $(MANIFEST) $(OUT)/
	cd $(OUT) && zip -r $(NAME).zip $(NAME) manifest.json

clean:
	rm -rf $(OUT)
	cargo clean