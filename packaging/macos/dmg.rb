# frozen_string_literal: true

# Package one already signed Spotidark app and the standard Applications link.
require "tmpdir"

payload, output = ARGV
abort "usage: dmg.rb PAYLOAD OUTPUT.dmg" unless payload && output
abort "output already exists: #{output}" if File.exist?(output)
abort "expected only Spotidark.app in the payload" unless Dir.children(payload) == ["Spotidark.app"]
Dir.mktmpdir("spotidark-dmg-") do |directory|
  app = File.join(payload, "Spotidark.app")
  abort "missing Spotidark.app" unless File.directory?(app)
  copied_app = File.join(directory, "Spotidark.app")
  abort "app copy failed" unless system("ditto", app, copied_app)
  abort "copied app signature failed" unless system("codesign", "--verify", "--deep", "--strict", copied_app)
  File.symlink("/Applications", File.join(directory, "Applications"))
  abort "DMG creation failed" unless system("hdiutil", "create", "-volname", "Spotidark",
    "-srcfolder", directory, "-format", "UDZO", output)
  abort "DMG verification failed" unless system("hdiutil", "verify", output)
end
