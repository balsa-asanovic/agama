# frozen_string_literal: true

require "yast"
require "y2storage"
require "yast2/installer_status"
require "yast2/software"
require "yast2/installation_progress"
require "bootloader/proposal_client"
require "bootloader/finish_client"
require "dbus"
require "forwardable"

Yast.import "Stage"

# YaST specific code lives under this namespace
module Yast2
  # This class represents the installer itself
  #
  # It is responsible for orchestrating the installation process. Additionally,
  # it serves as an entry point to other APIs.
  #
  # @example Get the current storage proposal
  #   installer = Installer.new
  #   installer.probe
  #   installer.storage_proposal #=> #<Y2Storage::Devicegraph>
  class Installer
    class InvalidValue < StandardError; end

    extend Forwardable

    DEFAULT_LANGUAGE = "en_US"

    attr_reader :disks, :languages
    attr_reader :disk
    attr_reader :logger
    attr_reader :language

    def_delegators :@software, :products, :product

    # @return [InstallerStatus]
    attr_reader :status

    # Returns a new instance of the Installer class
    #
    # @example Probe and run the installation
    #   installer = Installer.new
    #   installer.probe
    #   installer.install
    #
    # @example Reacting on status change
    #   installer = Installer.new
    #   installer.on_status_change do |status|
    #     log.info "Status changed: #{status}"
    #   end
    #
    # @param logger      [Logger,nil] Logger to write messages to
    def initialize(logger: nil)
      Yast::Mode.SetUI("commandline")
      Yast::Mode.SetMode("installation")
      @disks = []
      @languages = []
      @products = []
      @status = InstallerStatus::IDLE
      @logger = logger || Logger.new(STDOUT)
      @software = Software.new(@logger)
      # Set stage to initial, so it will act as installer for some cases like
      # proposing installer instead of reading current one
      Yast::Stage.Set("initial")
    end

    def options
      { "disk" => disk, "product" => product, "language" => language }
    end

    # Starts the probing process
    #
    # At this point, it just initializes some YaST modules/subsystems:
    #
    # * Software management
    # * Simplified storage probing
    #
    # The initialization of these subsystems should probably live in a different place.
    #
    # @return [Boolean] true if the probing process ended successfully; false otherwise.
    def probe
      change_status(InstallerStatus::PROBING)
      probe_languages
      @software.probe
      @software.propose
      probe_storage
      true
    rescue StandardError => e
      logger.error "Probing error: #{e.inspect}"
      false
    ensure
      change_status(InstallerStatus::IDLE)
    end

    def disk=(name)
      raise InvalidValue unless propose_storage(name)

      @disk = name
    end

    def product=(name)
      @software.select_product(name)
    rescue
      raise InvalidValue
    end

    def language=(name)
      raise InvalidValue unless languages.include?(name)

      @language = name
    end

    def storage_proposal
      storage_manager.proposal&.devices
    end

    def install
      change_status(InstallerStatus::INSTALLING)
      progress = InstallationProgress.new(@dbus_obj, logger: logger)
      Yast::Installation.destdir = "/mnt"
      progress.partitioning do |_|
        Yast::WFM.CallFunction("inst_prepdisk", [])
      end
      progress.package_installation do |progr|
        @software.install(progr)
      end
      progress.bootloader_installation do |_|
        proposal = ::Bootloader::ProposalClient.new.make_proposal({})
        logger.info "Bootloader proposal #{proposal.inspect}"
        ::Bootloader::FinishClient.new.write
      end
      change_status(InstallerStatus::IDLE)
    end

    # Callback to run when the status changes
    #
    # This callback receives the new InstallerStatus instance.
    #
    # @return block [Proc] Code to run when the status changes
    def on_status_change(&block)
      @on_status_change = block
    end

    def dbus_obj=(obj)
      @dbus_obj = obj
    end

  private

    def change_status(new_status)
      @status = new_status
      begin
        @on_status_change.call(new_status) if @on_status_change
      rescue ::DBus::Error
        # DBus object is not available yet
      end
    end

    # Returns the list of known languages
    #
    # @return [Hash]
    def probe_languages
      logger.info "Probing languages"
      Yast.import "Language"
      @languages = Yast::Language.GetLanguagesMap(true)
      self.language = DEFAULT_LANGUAGE
    end

    def probe_storage
      logger.info "Probing storage"
      storage_manager.probe
      @disks = storage_manager.probed.disks
      self.disk = @disks.first&.name
    end

    # @return [Boolean] true if success; false if failed
    def propose_storage(disk_name)
      settings = Y2Storage::ProposalSettings.new_for_current_product
      settings.candidate_devices = [disk_name]

      # FIXME: clean up the disks
      clean_probed = storage_probed.clone
      clean_probed.disks.each(&:remove_descendants)

      proposal = Y2Storage::GuidedProposal.initial(
        devicegraph: clean_probed,
        settings:    settings
      )
      return false if proposal.failed?

      storage_manager.proposal = proposal
      true
    end

    def storage_probed
      storage_manager.probed
    end

    def storage_manager
      @storage_manager ||= Y2Storage::StorageManager.instance
    end
  end
end
