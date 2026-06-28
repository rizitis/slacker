config() {
  NEW="$1"; OLD="$(dirname $NEW)/$(basename $NEW .new)"
  if [ ! -r $OLD ]; then
    mv $NEW $OLD
  elif [ "$(cat $OLD | md5sum)" = "$(cat $NEW | md5sum)" ]; then
    rm $NEW
  fi
}
config etc/slacker/slacker.conf.new
config etc/slacker/mirrors.new
config etc/slacker/repos.new
config etc/slacker/blacklist.new
config etc/slacker/distro-upgrade.conf
