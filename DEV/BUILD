sudo bash slacker.SlackBuild 2>&1 | tee "build-$(date +%Y%m%d-%H%M%S).log"
# --- build ---

# offline-friendly, optimized; release profile already does strip+lto
cargo build --release --offline 2>/dev/null || cargo build --release
mkdir -p $PKG/usr/sbin
install -m0755 target/release/slacker $PKG/usr/sbin/slacker
# docs
mkdir -p $PKG/usr/doc/$PRGNAM-$VERSION
install -m0644 README.md $PKG/usr/doc/$PRGNAM-$VERSION/
install -m0644 examples/etc-slacker/* $PKG/usr/doc/$PRGNAM-$VERSION/examples/



# online + debug is what we need for developing
cargo build
mkdir -p $PKG/usr/sbin
install -m0755 target/debug/slacker $PKG/usr/sbin/slacker
# docs
mkdir -p $PKG/usr/doc/$PRGNAM-$VERSION
install -m0644 README.md $PKG/usr/doc/$PRGNAM-$VERSION/
install -m0644 examples/etc-slacker/* $PKG/usr/doc/$PRGNAM-$VERSION/examples/



# config files — install as .new so an upgrade never clobbers the user's edits
mkdir -p $PKG/etc/slacker/templates
for f in slacker.conf mirrors repos blacklist; do
  install -m0644 examples/etc-slacker/$f $PKG/etc/slacker/$f.new
done

# man page
mkdir -p $PKG/usr/man/man8
install -m0644 man/slacker.8 $PKG/usr/man/man8/slacker.8
gzip -9 $PKG/usr/man/man8/slacker.8



