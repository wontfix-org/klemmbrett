
import sqlalchemy as _sa

def statement(options, plugin):
    return str(
        list(
            _sa.create_engine(options["engine"]).execute(
                options["statement"] % {'clipboard': plugin.history.top},
            )
        )[0][0]
    )
